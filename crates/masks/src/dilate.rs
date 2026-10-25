//! Binary dilation with OpenCV's elliptical structuring element, matching
//! `cv2.dilate(mask, cv2.getStructuringElement(cv2.MORPH_ELLIPSE, (2r+1, 2r+1)))`
//! as the M0 spike applied it to its masks.

/// Half-widths of OpenCV's `MORPH_ELLIPSE` kernel of size `(2r+1)²`, one per
/// kernel row `dy = -r..=r`: row `dy` covers columns `-dx..=dx`.
pub fn ellipse_half_widths(r: usize) -> Vec<usize> {
    // OpenCV getStructuringElement: inv_r2 = 1/r², dx = cvRound(c·√((r² - dy²)·inv_r2))
    // with c = r here; cvRound rounds half to even.
    let inv_r2 = if r == 0 { 0.0 } else { 1.0 / (r * r) as f64 };
    let r = r as isize;
    (-r..=r)
        .map(|dy| {
            let dx = r as f64 * (((r * r - dy * dy) as f64) * inv_r2).sqrt();
            dx.round_ties_even() as usize
        })
        .collect()
}

/// Dilate `mask` (row-major, `true` = set) with the elliptical kernel of
/// radius `r`. Pixels outside the image never contribute, as in OpenCV's
/// default border for dilation.
pub fn dilate_ellipse(mask: &[bool], width: usize, height: usize, r: usize) -> Vec<bool> {
    dilate_with(mask, width, height, &ellipse_half_widths(r))
}

/// Dilate with a symmetric kernel given by its row half-widths (`2r+1` rows).
pub(crate) fn dilate_with(mask: &[bool], width: usize, height: usize, half: &[usize]) -> Vec<bool> {
    assert_eq!(mask.len(), width * height, "mask size");
    let r = half.len() / 2;
    // Per-row prefix counts: "any set pixel in [x - dx, x + dx]" is O(1).
    let stride = width + 1;
    let mut prefix = vec![0u32; height * stride];
    for y in 0..height {
        for x in 0..width {
            prefix[y * stride + x + 1] = prefix[y * stride + x] + u32::from(mask[y * width + x]);
        }
    }
    let any_in_row = |y: usize, x: usize, dx: usize| {
        let (lo, hi) = (x.saturating_sub(dx), (x + dx + 1).min(width));
        prefix[y * stride + hi] > prefix[y * stride + lo]
    };
    let mut out = vec![false; width * height];
    for y in 0..height {
        for x in 0..width {
            out[y * width + x] = half.iter().enumerate().any(|(row, &dx)| {
                let sy = (y + row).checked_sub(r).filter(|&sy| sy < height);
                sy.is_some_and(|sy| any_in_row(sy, x, dx))
            });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use super::*;

    fn fixture(name: &str) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures/masks")
            .join(name)
    }

    /// (mask, width, height) from an 8-bit fixture PNG, set where > 0.
    fn load(name: &str) -> (Vec<bool>, usize, usize) {
        let img = image::open(fixture(name)).unwrap().into_luma8();
        let (w, h) = img.dimensions();
        (
            img.pixels().map(|p| p.0[0] > 0).collect(),
            w as usize,
            h as usize,
        )
    }

    fn kernel_pixels(half: &[usize]) -> usize {
        half.iter().map(|dx| 2 * dx + 1).sum()
    }

    #[test]
    fn kernel_sizes_match_opencv() {
        // From make_fixtures.py: kernel.sum() for radius 1, 3 and 8.
        assert_eq!(
            ellipse_half_widths(1),
            [0, 1, 0],
            "OpenCV's radius-1 ellipse is a cross"
        );
        assert_eq!(kernel_pixels(&ellipse_half_widths(3)), 33);
        assert_eq!(kernel_pixels(&ellipse_half_widths(8)), 213);
        assert_eq!(ellipse_half_widths(0), [0]);
    }

    #[test]
    fn dilation_matches_opencv_exactly() {
        let (mask, w, h) = load("mask.png");
        for r in [1, 3, 8] {
            let (want, ww, wh) = load(&format!("dilated_{r}.png"));
            assert_eq!((ww, wh), (w, h));
            let got = dilate_ellipse(&mask, w, h, r);
            let diff = got.iter().zip(&want).filter(|(a, b)| a != b).count();
            assert_eq!(diff, 0, "radius {r}: {diff} pixels differ from OpenCV");
        }
    }

    #[test]
    fn radius_zero_is_the_identity() {
        let (mask, w, h) = load("mask.png");
        assert_eq!(dilate_ellipse(&mask, w, h, 0), mask);
    }

    #[test]
    fn single_pixel_grows_into_the_kernel_and_clips_at_borders() {
        let (w, h) = (5, 4);
        let mut mask = vec![false; w * h];
        mask[0] = true; // top-left corner
        let got = dilate_ellipse(&mask, w, h, 1);
        let set: Vec<usize> = (0..w * h).filter(|&i| got[i]).collect();
        assert_eq!(
            set,
            [0, 1, w],
            "cross kernel, clipped at the top and left edges"
        );
    }

    /// The fixture must reject the plausible wrong kernel: half-widths by
    /// truncation instead of OpenCV's rounding.
    #[test]
    fn fixtures_reject_a_truncated_disk() {
        let (mask, w, h) = load("mask.png");
        let (want, _, _) = load("dilated_8.png");
        let r = 8usize;
        let floor: Vec<usize> = (0..=2 * r)
            .map(|i| ((r * r - (i as isize - r as isize).pow(2) as usize) as f64).sqrt() as usize)
            .collect();
        assert_ne!(
            floor,
            ellipse_half_widths(r),
            "variant must differ to be a test"
        );
        let wrong = dilate_with(&mask, w, h, &floor);
        assert!(wrong.iter().zip(&want).any(|(a, b)| a != b));
    }

    /// Same, for a square kernel (every row at full width).
    #[test]
    fn fixtures_reject_a_square_kernel() {
        let (mask, w, h) = load("mask.png");
        let (want, _, _) = load("dilated_3.png");
        let wrong = dilate_with(&mask, w, h, &[3; 7]);
        assert!(wrong.iter().zip(&want).any(|(a, b)| a != b));
    }
}
