//! `masks` stage: per-image masks of water, sky and transient objects. (M1)
//!
//! Masks follow the COLMAP convention: `images/00001.jpg` →
//! `masks/00001.jpg.png`. Black pixels are ignored by downstream stages.

use std::path::{Path, PathBuf};

/// Mask path for `image` (relative to the image root) under `mask_root`.
/// Subdirectories are preserved.
pub fn mask_path(mask_root: &Path, image: &Path) -> PathBuf {
    let mut name = image.as_os_str().to_owned();
    name.push(".png");
    mask_root.join(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn appends_png_to_full_file_name() {
        assert_eq!(
            mask_path(Path::new("masks"), Path::new("00001.jpg")),
            Path::new("masks/00001.jpg.png")
        );
    }

    #[test]
    fn preserves_subdirectories() {
        assert_eq!(
            mask_path(Path::new("masks"), Path::new("cam0/00001.JPG")),
            Path::new("masks/cam0/00001.JPG.png")
        );
    }
}
