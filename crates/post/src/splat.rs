//! Gaussian splat PLY (Brush/INRIA layout: one `vertex` element of `float`
//! properties, binary little endian) and the floater cull (ADR-0012).
//!
//! Rows are kept as raw bytes, so every property (SH coefficients, scales,
//! rotations) passes through unchanged; only `x`, `y` and `z` are decoded.

use std::path::Path;

use shoreline_core::error::io_error;
use shoreline_core::{Error, Result};
use shoreline_sfm::Model;

use crate::plane::{Plane, Vec3, dot, rotation};

#[derive(Debug, Clone, PartialEq)]
pub struct Splat {
    /// Header lines between `ply`/`format` and `element vertex` (comments).
    pub comments: Vec<String>,
    pub properties: Vec<String>,
    /// Row-major, `properties.len()` little-endian `f32` per Gaussian.
    pub data: Vec<u8>,
}

fn ply_error(path: &Path, message: impl std::fmt::Display) -> Error {
    Error::Stage {
        stage: crate::NAME,
        message: format!("{}: {message}", path.display()),
    }
}

impl Splat {
    pub fn read(path: &Path) -> Result<Self> {
        let bytes = std::fs::read(path).map_err(io_error(path))?;
        Self::parse(&bytes).map_err(|e| ply_error(path, e))
    }

    pub fn parse(bytes: &[u8]) -> std::result::Result<Self, String> {
        const END: &[u8] = b"end_header\n";
        let end = bytes
            .windows(END.len())
            .position(|w| w == END)
            .ok_or("no end_header")?
            + END.len();
        let header = std::str::from_utf8(&bytes[..end]).map_err(|_| "header is not UTF-8")?;
        let mut lines = header.lines();
        if lines.next() != Some("ply") {
            return Err("not a PLY file".into());
        }
        let mut comments = Vec::new();
        let mut properties = Vec::new();
        let mut count = None;
        for line in lines {
            let words: Vec<&str> = line.split_whitespace().collect();
            match words.as_slice() {
                ["format", "binary_little_endian", "1.0"] => {}
                ["format", ..] => return Err(format!("unsupported {line:?}")),
                ["comment", ..] => comments.push(line.to_owned()),
                ["element", "vertex", n] if count.is_none() => {
                    count = Some(n.parse::<usize>().map_err(|_| format!("bad count {n:?}"))?);
                }
                ["element", ..] => return Err(format!("unsupported {line:?}")),
                ["property", "float", name] => properties.push((*name).to_owned()),
                ["property", ..] => return Err(format!("unsupported {line:?}")),
                ["end_header"] => break,
                _ => return Err(format!("unexpected header line {line:?}")),
            }
        }
        let count = count.ok_or("no vertex element")?;
        for p in ["x", "y", "z"] {
            if !properties.iter().any(|q| q == p) {
                return Err(format!("no property {p}"));
            }
        }
        let stride = 4 * properties.len();
        let data = &bytes[end..];
        if data.len() != count * stride {
            return Err(format!(
                "{} data bytes for {count} vertices of {stride} bytes",
                data.len()
            ));
        }
        Ok(Self {
            comments,
            properties,
            data: data.to_vec(),
        })
    }

    pub fn len(&self) -> usize {
        self.data.len() / (4 * self.properties.len())
    }

    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }

    fn column(&self, name: &str) -> usize {
        self.properties
            .iter()
            .position(|p| p == name)
            .expect("x, y and z are checked on parse")
    }

    pub fn centres(&self) -> Vec<Vec3> {
        let stride = 4 * self.properties.len();
        let cols = ["x", "y", "z"].map(|n| 4 * self.column(n));
        self.data
            .chunks_exact(stride)
            .map(|row| {
                cols.map(|c| f32::from_le_bytes(row[c..c + 4].try_into().expect("4 bytes")) as f64)
            })
            .collect()
    }

    /// The Gaussians for which `keep[i]` holds.
    pub fn retain(&self, keep: &[bool]) -> Self {
        let stride = 4 * self.properties.len();
        let data = self
            .data
            .chunks_exact(stride)
            .zip(keep)
            .filter(|&(_, &k)| k)
            .flat_map(|(row, _)| row)
            .copied()
            .collect();
        Self {
            data,
            ..self.clone()
        }
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut header = String::from("ply\nformat binary_little_endian 1.0\n");
        for c in &self.comments {
            header.push_str(c);
            header.push('\n');
        }
        header.push_str(&format!("element vertex {}\n", self.len()));
        for p in &self.properties {
            header.push_str(&format!("property float {p}\n"));
        }
        header.push_str("end_header\n");
        let mut out = header.into_bytes();
        out.extend_from_slice(&self.data);
        out
    }

    pub fn write(&self, path: &Path) -> Result<()> {
        std::fs::write(path, self.encode()).map_err(io_error(path))
    }
}

/// Why each Gaussian was removed, in order of precedence.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize)]
pub struct CullCounts {
    pub below_water: usize,
    pub unseen: usize,
    pub over_water: usize,
    pub kept: usize,
}

/// Per Gaussian centre: the training views it projects into, and those in
/// which it lands on an ignored mask pixel while the ray to it points down
/// (towards the water plane, so not at the sky).
pub fn view_counts(
    centres: &[Vec3],
    model: &Model,
    masks: &Path,
    plane: &Plane,
) -> Result<(Vec<u32>, Vec<u32>)> {
    let mut seen = vec![0u32; centres.len()];
    let mut water = vec![0u32; centres.len()];
    for image in model.images.values() {
        let camera = &model.cameras[&image.camera_id];
        let (fx, fy, cx, cy) = match (camera.model, camera.params.as_slice()) {
            (0, &[f, cx, cy]) => (f, f, cx, cy),
            (1, &[fx, fy, cx, cy]) => (fx, fy, cx, cy),
            _ => {
                return Err(Error::Stage {
                    stage: crate::NAME,
                    message: format!(
                        "camera {} has model {}; the floater cull needs undistorted (pinhole) cameras",
                        image.camera_id, camera.model
                    ),
                });
            }
        };
        let path = masks.join(format!(
            "{}.png",
            shoreline_sfm::undistort::stem(&image.name)
        ));
        let mask = image::open(&path)
            .map_err(|e| ply_error(&path, e))?
            .into_luma8();
        let (w, h) = mask.dimensions();
        let r = rotation(image.rotation);
        let t = image.translation;
        // Camera centre: -R^T t.
        let centre = [0, 1, 2].map(|k| -(r[0][k] * t[0] + r[1][k] * t[1] + r[2][k] * t[2]));
        for (i, &p) in centres.iter().enumerate() {
            let z = dot(r[2], p) + t[2];
            if z <= 1e-6 {
                continue;
            }
            let u = fx * (dot(r[0], p) + t[0]) / z + cx;
            let v = fy * (dot(r[1], p) + t[1]) / z + cy;
            if !(u >= 0.0 && v >= 0.0 && u < w as f64 && v < h as f64) {
                continue;
            }
            seen[i] += 1;
            let ignored = mask.get_pixel(u as u32, v as u32)[0] < 128;
            let ray = [p[0] - centre[0], p[1] - centre[1], p[2] - centre[2]];
            if ignored && dot(plane.normal, ray) < 0.0 {
                water[i] += 1;
            }
        }
    }
    Ok((seen, water))
}

/// Keep a Gaussian if its centre is on or above the water plane, at least one
/// training view sees it, and at most `max_water` of those views see it
/// against masked water.
pub fn cull(
    centres: &[Vec3],
    seen: &[u32],
    water: &[u32],
    plane: &Plane,
    max_water: f64,
) -> (Vec<bool>, CullCounts) {
    let mut counts = CullCounts::default();
    let keep = centres
        .iter()
        .zip(seen.iter().zip(water))
        .map(|(&p, (&s, &w))| {
            if plane.signed_distance(p) < 0.0 {
                counts.below_water += 1;
                false
            } else if s == 0 {
                counts.unseen += 1;
                false
            } else if w as f64 > max_water * s as f64 {
                counts.over_water += 1;
                false
            } else {
                counts.kept += 1;
                true
            }
        })
        .collect();
    (keep, counts)
}

#[cfg(test)]
mod tests {
    use super::*;
    use shoreline_sfm::model::{Camera, Image};

    fn splat(points: &[[f32; 3]]) -> Splat {
        let properties: Vec<String> = ["x", "y", "z", "opacity"].map(String::from).into();
        let data = points
            .iter()
            .flat_map(|p| [p[0], p[1], p[2], 0.5])
            .flat_map(f32::to_le_bytes)
            .collect();
        Splat {
            comments: vec!["comment Exported from Brush".into()],
            properties,
            data,
        }
    }

    #[test]
    fn encode_parse_round_trip_and_retain() {
        let s = splat(&[[1.0, 2.0, 3.0], [4.0, 5.0, 6.0]]);
        let back = Splat::parse(&s.encode()).unwrap();
        assert_eq!(back, s);
        assert_eq!(back.centres(), [[1.0, 2.0, 3.0], [4.0, 5.0, 6.0]]);
        let one = s.retain(&[false, true]);
        assert_eq!(one.len(), 1);
        assert_eq!(one.centres(), [[4.0, 5.0, 6.0]]);
        assert!(String::from_utf8_lossy(&one.encode()).contains("element vertex 1\n"));
    }

    #[test]
    fn parse_rejects_other_layouts() {
        assert!(
            Splat::parse(
                b"ply\nformat ascii 1.0\nelement vertex 0\nproperty float x\nend_header\n"
            )
            .is_err()
        );
        assert!(Splat::parse(b"ply\nformat binary_little_endian 1.0\nelement vertex 1\nproperty float x\nproperty float y\nproperty float z\nend_header\n").is_err());
        assert!(Splat::parse(b"ply\nformat binary_little_endian 1.0\nelement vertex 0\nproperty uchar x\nend_header\n").is_err());
    }

    #[test]
    fn cull_removes_below_water_unseen_and_over_water_in_that_order() {
        let plane = Plane {
            normal: [0.0, 0.0, 1.0],
            height: 0.0,
        };
        let centres = [
            [0.0, 0.0, -1.0],
            [0.0, 0.0, 1.0],
            [0.0, 0.0, 1.0],
            [0.0, 0.0, 1.0],
        ];
        let seen = [5, 0, 4, 4];
        let water = [5, 0, 3, 2];
        let (keep, counts) = cull(&centres, &seen, &water, &plane, 0.5);
        assert_eq!(keep, [false, false, false, true]);
        assert_eq!(
            counts,
            CullCounts {
                below_water: 1,
                unseen: 1,
                over_water: 1,
                kept: 1
            }
        );
    }

    #[test]
    fn view_counts_count_water_only_on_downward_rays_into_ignored_pixels() {
        // One pinhole camera at the origin looking along +Y (world up = +Z):
        // camera x = +X, y = -Z, z = +Y. Image 100x100, f = 50, centre 50.
        let q = {
            let (s, c) = (std::f64::consts::FRAC_PI_4).sin_cos();
            [c, s, 0.0, 0.0]
        };
        let model = Model {
            cameras: [(
                1,
                Camera {
                    model: 1,
                    width: 100,
                    height: 100,
                    params: vec![50.0, 50.0, 50.0, 50.0],
                },
            )]
            .into_iter()
            .collect(),
            images: [(
                1,
                Image {
                    rotation: q,
                    translation: [0.0; 3],
                    camera_id: 1,
                    name: "a.jpg".into(),
                    points2d: Vec::new(),
                },
            )]
            .into_iter()
            .collect(),
            ..Model::default()
        };
        // Whole mask ignored: the lower half is water, the upper half sky.
        let tmp = tempfile::tempdir().unwrap();
        image::GrayImage::new(100, 100)
            .save(tmp.path().join("a.png"))
            .unwrap();
        let plane = Plane {
            normal: [0.0, 0.0, 1.0],
            height: -10.0,
        };
        // Ahead and below eye level, ahead and above, and behind the camera.
        let centres = [[0.0, 10.0, -2.0], [0.0, 10.0, 2.0], [0.0, -10.0, 0.0]];
        let (seen, water) = view_counts(&centres, &model, tmp.path(), &plane).unwrap();
        assert_eq!(seen, [1, 1, 0]);
        assert_eq!(water, [1, 0, 0]);
    }
}
