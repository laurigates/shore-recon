//! Near-duplicate frame removal for the frames stage (#10).
//!
//! Frames are compared by an 8x8 average perceptual hash (aHash); a frame
//! whose hash is within `threshold` Hamming bits of *any* previously kept
//! frame is dropped (all kept frames are compared, not just the immediate
//! predecessor). This is cheap, scale/shift tolerant enough for consecutive
//! video frames, and needs no external tools.

use std::path::Path;

use shoreline_core::Result;
use shoreline_core::error::io_error;

use crate::blur::{Luma, decode_luma};

/// An 8x8 average hash of a luma plane, packed into 64 bits, row-major.
pub fn ahash(luma: &Luma) -> u64 {
    let (w, h) = (luma.width, luma.height);
    if w == 0 || h == 0 {
        return 0;
    }
    // Sample one pixel per 8x8 cell (nearest neighbour), then threshold
    // against the mean. Point sampling keeps more high-frequency detail
    // than box-averaging, which matters for telling distinct frames apart.
    let mut cells = [0.0f64; 64];
    for (idx, cell) in cells.iter_mut().enumerate() {
        let (cx, cy) = (idx % 8, idx / 8);
        let x = (cx * w / 8).min(w.saturating_sub(1));
        let y = (cy * h / 8).min(h.saturating_sub(1));
        *cell = f64::from(luma.pixels[y * w + x]);
    }
    let mean: f64 = cells.iter().sum::<f64>() / cells.len() as f64;
    let mut hash = 0u64;
    for (i, cell) in cells.iter().enumerate() {
        if *cell >= mean {
            hash |= 1u64 << i;
        }
    }
    hash
}

/// Hamming distance between two 64-bit hashes.
pub fn hamming_distance(a: u64, b: u64) -> u32 {
    (a ^ b).count_ones()
}

/// Drop frames in `images` that are near-duplicates of an earlier kept frame.
/// Returns the names of the dropped frames. Dropped files are moved into
/// `rejected`.
pub fn drop_duplicates(images: &Path, rejected: &Path, threshold: u32) -> Result<Vec<String>> {
    let mut names = Vec::new();
    for entry in std::fs::read_dir(images).map_err(io_error(images))? {
        let name = entry.map_err(io_error(images))?.file_name();
        if let Some(name) = name.to_str().filter(|n| n.ends_with(".jpg")) {
            names.push(name.to_owned());
        }
    }
    names.sort_by(|a, b| (a.len(), a).cmp(&(b.len(), b)));
    if names.is_empty() {
        return Ok(Vec::new());
    }

    let mut kept_hashes: Vec<u64> = Vec::new();
    let mut dropped = Vec::new();
    for name in names {
        let path = images.join(&name);
        let luma = decode_luma(&path)?;
        let hash = ahash(&luma);
        let is_dup = kept_hashes
            .iter()
            .any(|h| hamming_distance(*h, hash) <= threshold);
        if is_dup {
            std::fs::create_dir_all(rejected).map_err(io_error(rejected))?;
            std::fs::rename(&path, rejected.join(&name)).map_err(io_error(&path))?;
            dropped.push(name);
        } else {
            kept_hashes.push(hash);
        }
    }
    Ok(dropped)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn fixture(name: &str) -> std::path::PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures/frames")
            .join(name)
    }

    #[test]
    fn identical_frames_have_zero_distance() {
        let l = decode_luma(&fixture("sharp.jpg")).unwrap();
        let h = ahash(&l);
        assert_eq!(hamming_distance(h, h), 0);
    }

    #[test]
    fn hamming_counts_differing_bits() {
        assert_eq!(hamming_distance(0b0000, 0b1111), 4);
        assert_eq!(hamming_distance(0u64, !0u64), 64);
    }

    #[test]
    fn drop_duplicates_moves_exact_copies() {
        let dir = tempfile::tempdir().unwrap();
        let (images, rejected) = (dir.path().join("images"), dir.path().join("rejected"));
        fs::create_dir(&images).unwrap();
        // sharp.jpg twice, plus a different frame.
        for i in 0..2 {
            fs::copy(
                fixture("sharp.jpg"),
                images.join(format!("{:05}.jpg", i + 1)),
            )
            .unwrap();
        }
        fs::copy(fixture("blurry.jpg"), images.join("00003.jpg")).unwrap();

        let dropped = drop_duplicates(&images, &rejected, 0).unwrap();
        assert_eq!(dropped, vec!["00002.jpg".to_string()]);
        assert!(images.join("00001.jpg").is_file() && images.join("00003.jpg").is_file());
        assert!(rejected.join("00002.jpg").is_file());
    }

    #[test]
    fn threshold_zero_keeps_only_exact_duplicates_dropped() {
        let dir = tempfile::tempdir().unwrap();
        let (images, rejected) = (dir.path().join("images"), dir.path().join("rejected"));
        fs::create_dir(&images).unwrap();
        // An exact duplicate of the first frame, plus a differing frame.
        fs::copy(fixture("sharp.jpg"), images.join("00001.jpg")).unwrap();
        fs::copy(fixture("sharp.jpg"), images.join("00002.jpg")).unwrap();
        fs::copy(fixture("soft.jpg"), images.join("00003.jpg")).unwrap();
        let dropped = drop_duplicates(&images, &rejected, 0).unwrap();
        // At threshold 0 an exact duplicate is dropped...
        assert_eq!(dropped, vec!["00002.jpg".to_string()]);
        // ...and a differing frame is kept.
        assert!(images.join("00001.jpg").is_file() && images.join("00003.jpg").is_file());
        assert!(rejected.join("00002.jpg").is_file());
    }
}
