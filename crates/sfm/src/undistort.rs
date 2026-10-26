//! Undistortion for the dense and splat backends, which read COLMAP-undistorted
//! PINHOLE images (ADR-0011).
//!
//! `colmap image_undistorter` writes the images and model. Masks are warped
//! here with the same pixel mapping as COLMAP 4.2.1's `WarpImageBetweenCameras`
//! (direct warp, nearest neighbour): for each target pixel centre
//! `(x + 0.5, y + 0.5)`, `CamFromImg` on the undistorted camera, `ImgFromCam` on
//! the distorted one, then the source pixel `round(p - 0.5)`. Pixels that map
//! outside the source are 0, which in a mask means ignore.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use shoreline_core::error::io_error;
use shoreline_core::{Error, Result};

use crate::model::{Camera, Model};

/// `colmap image_undistorter` arguments writing `out/{images,sparse,stereo}`.
pub fn undistorter_args(images: &Path, sparse: &Path, out: &Path) -> Vec<OsString> {
    let mut args: Vec<OsString> = vec!["image_undistorter".into(), "--image_path".into()];
    args.extend([
        images.into(),
        "--input_path".into(),
        sparse.into(),
        "--output_path".into(),
    ]);
    args.extend([out.into(), "--output_type".into(), "COLMAP".into()]);
    args
}

const SIMPLE_PINHOLE: i32 = 0;
const PINHOLE: i32 = 1;
const SIMPLE_RADIAL: i32 = 2;
const RADIAL: i32 = 3;

/// Pixel to normalized camera coordinates; undistorted (pinhole) cameras only.
fn cam_from_img(c: &Camera, [x, y]: [f64; 2]) -> Option<[f64; 2]> {
    let p = &c.params;
    match c.model {
        SIMPLE_PINHOLE => Some([(x - p[1]) / p[0], (y - p[2]) / p[0]]),
        PINHOLE => Some([(x - p[2]) / p[0], (y - p[3]) / p[1]]),
        _ => None,
    }
}

/// Normalized camera coordinates to pixel, with distortion.
fn img_from_cam(c: &Camera, [u, v]: [f64; 2]) -> Option<[f64; 2]> {
    let p = &c.params;
    let r2 = u * u + v * v;
    let (f, cx, cy, radial) = match c.model {
        SIMPLE_PINHOLE => (p[0], p[1], p[2], 0.0),
        PINHOLE => return Some([p[0] * u + p[2], p[1] * v + p[3]]),
        SIMPLE_RADIAL => (p[0], p[1], p[2], p[3] * r2),
        RADIAL => (p[0], p[1], p[2], p[3] * r2 + p[4] * r2 * r2),
        _ => return None,
    };
    Some([f * (u + u * radial) + cx, f * (v + v * radial) + cy])
}

/// For each target pixel (row-major), the source pixel index it samples.
pub fn warp_map(source: &Camera, target: &Camera) -> Result<Vec<Option<usize>>> {
    let unsupported = |c: &Camera, role| {
        undistort_error(format!(
            "camera model {} is not supported as the {role} camera",
            c.model
        ))
    };
    if cam_from_img(target, [0.0, 0.0]).is_none() {
        return Err(unsupported(target, "undistorted"));
    }
    if img_from_cam(source, [0.0, 0.0]).is_none() {
        return Err(unsupported(source, "distorted"));
    }
    let (sw, sh) = (source.width as i64, source.height as i64);
    let (tw, th) = (target.width as usize, target.height as usize);
    let mut map = Vec::with_capacity(tw * th);
    for y in 0..th {
        for x in 0..tw {
            let cam = cam_from_img(target, [x as f64 + 0.5, y as f64 + 0.5]).expect("checked");
            let [sx, sy] = img_from_cam(source, cam).expect("checked");
            let (ix, iy) = ((sx - 0.5).round() as i64, (sy - 0.5).round() as i64);
            let inside = (0..sw).contains(&ix) && (0..sh).contains(&iy);
            map.push(inside.then(|| (iy * sw + ix) as usize));
        }
    }
    Ok(map)
}

/// Warp every registered image's mask (`masks/<image name>.png`, the COLMAP
/// convention) into the undistorted model's cameras, writing
/// `out/<rename(image name)>`. Returns the number of masks written.
pub fn undistort_masks(
    distorted: &Model,
    undistorted: &Model,
    masks: &Path,
    out: &Path,
    rename: impl Fn(&str) -> String,
) -> Result<usize> {
    std::fs::create_dir_all(out).map_err(io_error(out))?;
    let mut maps = std::collections::BTreeMap::new();
    for (id, image) in &undistorted.images {
        let Some(source) = distorted.images.get(id).filter(|s| s.name == image.name) else {
            return Err(undistort_error(format!(
                "image {id} ({}) is not in the distorted model",
                image.name
            )));
        };
        let (sc, tc) = (
            &distorted.cameras[&source.camera_id],
            &undistorted.cameras[&image.camera_id],
        );
        let key = (source.camera_id, image.camera_id);
        if let std::collections::btree_map::Entry::Vacant(e) = maps.entry(key) {
            e.insert(warp_map(sc, tc)?);
        }
        let path = masks.join(format!("{}.png", image.name));
        let mask = image::open(&path)
            .map_err(|e| undistort_error(format!("{}: {e}", path.display())))?
            .into_luma8();
        if (mask.width() as u64, mask.height() as u64) != (sc.width, sc.height) {
            return Err(undistort_error(format!(
                "{} is {}x{} but camera {} is {}x{}",
                path.display(),
                mask.width(),
                mask.height(),
                source.camera_id,
                sc.width,
                sc.height
            )));
        }
        let src = mask.as_raw();
        let warped: Vec<u8> = maps[&key].iter().map(|i| i.map_or(0, |i| src[i])).collect();
        let target = out.join(rename(&image.name));
        image::GrayImage::from_raw(tc.width as u32, tc.height as u32, warped)
            .expect("map has one entry per target pixel")
            .save(&target)
            .map_err(|e| undistort_error(format!("{}: {e}", target.display())))?;
    }
    Ok(undistorted.images.len())
}

/// `00001.jpg` → `00001` (the file stem, as Brush and OpenMVS match masks).
pub fn stem(name: &str) -> String {
    Path::new(name)
        .file_stem()
        .map_or_else(|| name.to_owned(), |s| s.to_string_lossy().into_owned())
}

/// The undistorter writes the model files (`*.bin`, including COLMAP 4's
/// `rigs.bin` and `frames.bin`) to `sparse/`; Brush expects `sparse/0/`.
pub fn sparse_to_sparse0(dir: &Path) -> Result<PathBuf> {
    let (sparse, zero) = (dir.join("sparse"), dir.join("sparse/0"));
    std::fs::create_dir_all(&zero).map_err(io_error(&zero))?;
    for entry in std::fs::read_dir(&sparse).map_err(io_error(&sparse))? {
        let from = entry.map_err(io_error(&sparse))?.path();
        if from.extension().is_some_and(|e| e == "bin") {
            let to = zero.join(from.file_name().expect("read_dir entries have names"));
            std::fs::rename(&from, to).map_err(io_error(&from))?;
        }
    }
    Ok(zero)
}

fn undistort_error(message: String) -> Error {
    Error::Stage {
        stage: crate::NAME,
        message: format!("undistort: {message}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::tests::sample;

    fn cam(model: i32, width: u64, height: u64, params: &[f64]) -> Camera {
        Camera {
            model,
            width,
            height,
            params: params.to_vec(),
        }
    }

    #[test]
    fn undistorter_args_write_a_colmap_workspace() {
        let args = undistorter_args(
            Path::new("/f/images"),
            Path::new("/s/sparse/0"),
            Path::new("/o/u"),
        );
        let args: Vec<_> = args
            .iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            args,
            [
                "image_undistorter",
                "--image_path",
                "/f/images",
                "--input_path",
                "/s/sparse/0",
                "--output_path",
                "/o/u",
                "--output_type",
                "COLMAP"
            ]
        );
    }

    #[test]
    fn identical_pinhole_cameras_map_each_pixel_to_itself() {
        let c = cam(PINHOLE, 7, 5, &[10.0, 10.0, 3.5, 2.5]);
        let map = warp_map(&c, &c).unwrap();
        assert_eq!(map, (0..35).map(Some).collect::<Vec<_>>());
    }

    #[test]
    fn simple_radial_with_k_zero_is_a_pinhole() {
        let radial = cam(SIMPLE_RADIAL, 9, 6, &[12.0, 4.5, 3.0, 0.0]);
        let pinhole = cam(PINHOLE, 9, 6, &[12.0, 12.0, 4.5, 3.0]);
        assert_eq!(
            warp_map(&radial, &pinhole).unwrap(),
            warp_map(&pinhole, &pinhole).unwrap()
        );
    }

    #[test]
    fn barrel_distortion_pulls_corners_from_further_out() {
        // k > 0 pushes source points outward, so a target corner samples beyond the
        // source frame (ignored) while the centre stays put.
        let radial = cam(SIMPLE_RADIAL, 101, 101, &[50.0, 50.5, 50.5, 0.2]);
        let pinhole = cam(PINHOLE, 101, 101, &[50.0, 50.0, 50.5, 50.5]);
        let map = warp_map(&radial, &pinhole).unwrap();
        assert_eq!(map[50 * 101 + 50], Some(50 * 101 + 50));
        assert_eq!(map[0], None);
        // Analytic check of one interior pixel: target (70.5, 50.5) is u = 0.4, v = 0.
        let x = 50.0 * (0.4 + 0.4 * 0.2 * 0.16) + 50.5;
        assert_eq!(
            map[50 * 101 + 70],
            Some(50 * 101 + (x - 0.5_f64).round() as usize)
        );
    }

    #[test]
    fn unsupported_models_are_errors() {
        let opencv = cam(4, 4, 4, &[1.0; 8]);
        let pinhole = cam(PINHOLE, 4, 4, &[1.0, 1.0, 2.0, 2.0]);
        assert!(warp_map(&opencv, &pinhole).is_err());
        assert!(warp_map(&pinhole, &opencv).is_err());
    }

    #[test]
    fn masks_are_warped_renamed_and_sized_by_the_undistorted_camera() {
        let dir = tempfile::tempdir().unwrap();
        let (masks, out) = (dir.path().join("masks"), dir.path().join("out"));
        std::fs::create_dir_all(&masks).unwrap();
        let mut distorted = sample();
        distorted
            .cameras
            .insert(1, cam(SIMPLE_RADIAL, 8, 6, &[10.0, 4.0, 3.0, 0.0]));
        let mut undistorted = distorted.clone();
        undistorted
            .cameras
            .insert(1, cam(PINHOLE, 8, 6, &[10.0, 10.0, 4.0, 3.0]));
        for image in distorted.images.values() {
            let mut m = image::GrayImage::new(8, 6);
            m.put_pixel(2, 1, image::Luma([255]));
            m.save(masks.join(format!("{}.png", image.name))).unwrap();
        }
        let n = undistort_masks(&distorted, &undistorted, &masks, &out, |n| {
            format!("{}.mask.png", stem(n))
        })
        .unwrap();
        assert_eq!(n, 2);
        let warped = image::open(out.join("f_0001.mask.png"))
            .unwrap()
            .into_luma8();
        assert_eq!(warped.dimensions(), (8, 6));
        assert_eq!(warped.get_pixel(2, 1).0, [255]);
        assert_eq!(warped.pixels().filter(|p| p.0[0] == 255).count(), 1);
    }

    #[test]
    fn a_mask_of_the_wrong_size_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let masks = dir.path().join("masks");
        std::fs::create_dir_all(&masks).unwrap();
        let mut model = sample();
        model
            .cameras
            .insert(1, cam(PINHOLE, 8, 6, &[10.0, 10.0, 4.0, 3.0]));
        for image in model.images.values() {
            image::GrayImage::new(4, 4)
                .save(masks.join(format!("{}.png", image.name)))
                .unwrap();
        }
        let err = undistort_masks(&model, &model, &masks, &dir.path().join("o"), stem).unwrap_err();
        assert!(
            err.to_string().contains("is 4x4 but camera 1 is 8x6"),
            "{err}"
        );
    }

    #[test]
    fn sparse_model_files_move_into_sparse_0() {
        let dir = tempfile::tempdir().unwrap();
        let sparse = dir.path().join("sparse");
        std::fs::create_dir_all(&sparse).unwrap();
        for name in [
            "cameras.bin",
            "images.bin",
            "points3D.bin",
            "rigs.bin",
            "frames.bin",
            "project.ini",
        ] {
            std::fs::write(sparse.join(name), b"").unwrap();
        }
        let zero = sparse_to_sparse0(dir.path()).unwrap();
        let mut moved: Vec<_> = std::fs::read_dir(&zero)
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        moved.sort();
        assert_eq!(
            moved,
            [
                "cameras.bin",
                "frames.bin",
                "images.bin",
                "points3D.bin",
                "rigs.bin"
            ]
        );
        assert!(sparse.join("project.ini").exists());
    }

    #[test]
    fn stems_drop_the_last_extension() {
        assert_eq!(stem("00001.jpg"), "00001");
        assert_eq!(stem("a.b.jpg"), "a.b");
    }
}
