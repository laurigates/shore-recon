//! Water plane estimation (ADR-0012).
//!
//! The normal is the gravity direction from the cameras: a drone gimbal keeps
//! each camera's x axis level, so up is the direction orthogonal to all of
//! them. The height is the lowest dense cluster of waterline points: sparse
//! points that, in most of the views that observe them, sit just above a
//! masked region of the image. Water lies below its boundary in an image and
//! sky above, so the band only looks downwards. The deck edges of a pier also
//! border water from above in steep views; they form a higher cluster, which
//! the lowest-mode rule skips.

use std::collections::HashMap;
use std::path::Path;

use shoreline_core::{Error, Result};
use shoreline_sfm::Model;

pub type Vec3 = [f64; 3];

pub fn dot(a: Vec3, b: Vec3) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

pub fn cross(a: Vec3, b: Vec3) -> Vec3 {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn scale(a: Vec3, s: f64) -> Vec3 {
    a.map(|x| x * s)
}

fn sub(a: Vec3, b: Vec3) -> Vec3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

pub fn normalize(a: Vec3) -> Vec3 {
    scale(a, 1.0 / dot(a, a).sqrt())
}

/// Rows of the rotation matrix of a unit quaternion `[w, x, y, z]`.
pub fn rotation(q: [f64; 4]) -> [Vec3; 3] {
    let [w, x, y, z] = q;
    [
        [
            1.0 - 2.0 * (y * y + z * z),
            2.0 * (x * y - w * z),
            2.0 * (x * z + w * y),
        ],
        [
            2.0 * (x * y + w * z),
            1.0 - 2.0 * (x * x + z * z),
            2.0 * (y * z - w * x),
        ],
        [
            2.0 * (x * z - w * y),
            2.0 * (y * z + w * x),
            1.0 - 2.0 * (x * x + y * y),
        ],
    ]
}

/// Eigenvector of the smallest eigenvalue of a symmetric 3x3 matrix (cyclic
/// Jacobi rotations).
fn smallest_eigenvector(mut a: [Vec3; 3]) -> Vec3 {
    let mut v = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
    for _ in 0..50 {
        let off = a[0][1].powi(2) + a[0][2].powi(2) + a[1][2].powi(2);
        if off < 1e-30 {
            break;
        }
        for (p, q) in [(0, 1), (0, 2), (1, 2)] {
            if a[p][q].abs() < 1e-300 {
                continue;
            }
            let theta = (a[q][q] - a[p][p]) / (2.0 * a[p][q]);
            let t = theta.signum() / (theta.abs() + (theta * theta + 1.0).sqrt());
            let t = if theta == 0.0 { 1.0 } else { t };
            let c = 1.0 / (t * t + 1.0).sqrt();
            let s = t * c;
            // A <- J^T A J, with J the rotation in the (p, q) plane.
            for row in &mut a {
                let (akp, akq) = (row[p], row[q]);
                row[p] = c * akp - s * akq;
                row[q] = s * akp + c * akq;
            }
            let (ap, aq) = (a[p], a[q]);
            a[p] = [0, 1, 2].map(|k| c * ap[k] - s * aq[k]);
            a[q] = [0, 1, 2].map(|k| s * ap[k] + c * aq[k]);
            for row in &mut v {
                let (vp, vq) = (row[p], row[q]);
                row[p] = c * vp - s * vq;
                row[q] = s * vp + c * vq;
            }
        }
    }
    let i = (0..3)
        .min_by(|&i, &j| a[i][i].total_cmp(&a[j][j]))
        .expect("three diagonal entries");
    [v[0][i], v[1][i], v[2][i]]
}

/// Up direction from the cameras' x axes, and the largest angle (degrees)
/// between any camera's x axis and the horizontal plane it defines.
pub fn camera_up(model: &Model) -> Result<(Vec3, f64)> {
    if model.images.len() < 2 {
        return Err(plane_error("at least two registered images are needed"));
    }
    let rows: Vec<[Vec3; 3]> = model
        .images
        .values()
        .map(|i| rotation(i.rotation))
        .collect();
    let mut m = [[0.0; 3]; 3];
    for r in &rows {
        let x = r[0];
        for i in 0..3 {
            for j in 0..3 {
                m[i][j] += x[i] * x[j];
            }
        }
    }
    let mut up = normalize(smallest_eigenvector(m));
    // Camera y points down in the image, so it should point against up.
    if rows.iter().map(|r| dot(r[1], up)).sum::<f64>() > 0.0 {
        up = scale(up, -1.0);
    }
    let tilt = rows
        .iter()
        .map(|r| dot(r[0], up).abs().asin().to_degrees())
        .fold(0.0, f64::max);
    Ok((up, tilt))
}

/// `true` for kept pixels that have an ignored pixel within `band` rows below.
pub fn waterline_pixels(mask: &image::GrayImage, band: u32) -> Vec<bool> {
    let (w, h) = mask.dimensions();
    let mut out = vec![false; (w * h) as usize];
    for x in 0..w {
        // Rows since the last ignored pixel, scanning upwards.
        let mut since = u32::MAX;
        for y in (0..h).rev() {
            let kept = mask.get_pixel(x, y)[0] >= 128;
            if kept {
                since = since.saturating_add(1);
                out[(y * w + x) as usize] = since <= band;
            } else {
                since = 0;
            }
        }
    }
    out
}

/// Sparse points on the waterline: observed at least twice, at least half of
/// the time on a waterline pixel of `masks/<image name>.png`.
pub fn waterline_points(model: &Model, masks: &Path, band: u32) -> Result<Vec<Vec3>> {
    let mut hits: HashMap<u64, u32> = HashMap::new();
    for image in model.images.values() {
        let path = masks.join(format!("{}.png", image.name));
        let mask = image::open(&path)
            .map_err(|e| plane_error(format!("{}: {e}", path.display())))?
            .into_luma8();
        let (w, h) = mask.dimensions();
        let line = waterline_pixels(&mask, band);
        for p in &image.points2d {
            let Some(id) = p.point3d else { continue };
            let [x, y] = p.xy;
            if x < 0.0 || y < 0.0 || x >= w as f64 || y >= h as f64 {
                continue;
            }
            if line[(y as u32 * w + x as u32) as usize] {
                *hits.entry(id).or_default() += 1;
            }
        }
    }
    let mut points: Vec<(u64, Vec3)> = hits
        .into_iter()
        .filter_map(|(id, n)| {
            let p = &model.points3d[&id];
            (n >= 2 && 2 * n as usize >= p.track.len()).then_some((id, p.xyz))
        })
        .collect();
    points.sort_by_key(|&(id, _)| id);
    Ok(points.into_iter().map(|(_, p)| p).collect())
}

/// The lowest local maximum of a Gaussian kernel density of `values` that
/// reaches `min_peak` of the highest, refined to the median of the values
/// within two bandwidths of it. `None` for no values.
pub fn lowest_mode(values: &[f64], bandwidth: f64, min_peak: f64) -> Option<f64> {
    let lo = values.iter().copied().fold(f64::INFINITY, f64::min);
    let hi = values.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    if !lo.is_finite() {
        return None;
    }
    const STEPS: usize = 2000;
    let grid: Vec<f64> = (0..=STEPS)
        .map(|i| lo - 3.0 * bandwidth + (hi - lo + 6.0 * bandwidth) * i as f64 / STEPS as f64)
        .collect();
    let density: Vec<f64> = grid
        .iter()
        .map(|g| {
            values
                .iter()
                .map(|v| (-0.5 * ((g - v) / bandwidth).powi(2)).exp())
                .sum()
        })
        .collect();
    let max = density.iter().copied().fold(0.0, f64::max);
    let peak = (1..STEPS).find(|&i| {
        density[i] >= density[i - 1] && density[i] > density[i + 1] && density[i] >= min_peak * max
    })?;
    let mut near: Vec<f64> = values
        .iter()
        .copied()
        .filter(|v| (v - grid[peak]).abs() <= 2.0 * bandwidth)
        .collect();
    near.sort_by(f64::total_cmp);
    Some(near[near.len() / 2])
}

/// Median distance of the points from their coordinate-wise median: a scale
/// for the scene that does not depend on its unit.
pub fn spread(points: &[Vec3]) -> f64 {
    let median = |mut v: Vec<f64>| {
        v.sort_by(f64::total_cmp);
        v[v.len() / 2]
    };
    let c = [0, 1, 2].map(|k| median(points.iter().map(|p| p[k]).collect()));
    median(
        points
            .iter()
            .map(|p| dot(sub(*p, c), sub(*p, c)).sqrt())
            .collect(),
    )
}

/// The water plane `{x : normal · x = height}` and the model frame built on
/// it: `+Y` is the normal and the origin is on the plane.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize)]
pub struct Plane {
    pub normal: Vec3,
    pub height: f64,
}

impl Plane {
    pub fn signed_distance(&self, p: Vec3) -> f64 {
        dot(self.normal, p) - self.height
    }
}

/// A rigid transform from the reconstruction frame to the model frame,
/// `local = rows · (world − origin)`.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize)]
pub struct Frame {
    pub origin: Vec3,
    /// Rows: model X, Y (up) and Z in reconstruction coordinates.
    pub rows: [Vec3; 3],
}

impl Frame {
    /// The frame on `plane` whose X axis is `right` projected onto the plane.
    /// `origin` is projected onto the plane.
    pub fn on_plane(plane: &Plane, origin: Vec3, right: Vec3) -> Self {
        let n = plane.normal;
        let ex = normalize(sub(right, scale(n, dot(right, n))));
        let ez = cross(ex, n);
        let origin = sub(origin, scale(n, plane.signed_distance(origin)));
        Self {
            origin,
            rows: [ex, n, ez],
        }
    }

    pub fn to_local(&self, p: Vec3) -> Vec3 {
        let d = sub(p, self.origin);
        self.rows.map(|r| dot(r, d))
    }

    /// Row-major 4x4 matrix of [`Frame::to_local`].
    pub fn matrix(&self) -> [[f64; 4]; 4] {
        let t = self.rows.map(|r| -dot(r, self.origin));
        let r = self.rows;
        [
            [r[0][0], r[0][1], r[0][2], t[0]],
            [r[1][0], r[1][1], r[1][2], t[1]],
            [r[2][0], r[2][1], r[2][2], t[2]],
            [0.0, 0.0, 0.0, 1.0],
        ]
    }
}

fn plane_error(message: impl Into<String>) -> Error {
    Error::Stage {
        stage: crate::NAME,
        message: message.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use shoreline_sfm::model::{Image, Point2D, Point3D};
    use std::collections::BTreeMap;

    fn approx(a: Vec3, b: Vec3, tol: f64) -> bool {
        (0..3).all(|k| (a[k] - b[k]).abs() < tol)
    }

    /// Quaternion `[w, x, y, z]` of a rotation by `angle` about unit `axis`.
    fn quat(axis: Vec3, angle: f64) -> [f64; 4] {
        let (s, c) = (angle / 2.0).sin_cos();
        [c, axis[0] * s, axis[1] * s, axis[2] * s]
    }

    fn qmul(a: [f64; 4], b: [f64; 4]) -> [f64; 4] {
        let [aw, ax, ay, az] = a;
        let [bw, bx, by, bz] = b;
        [
            aw * bw - ax * bx - ay * by - az * bz,
            aw * bx + ax * bw + ay * bz - az * by,
            aw * by - ax * bz + ay * bw + az * bx,
            aw * bz + ax * by - ay * bx + az * bw,
        ]
    }

    #[test]
    fn smallest_eigenvector_of_a_diagonal_and_a_rotated_matrix() {
        let e = smallest_eigenvector([[3.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 2.0]]);
        assert!(approx(e.map(f64::abs), [0.0, 1.0, 0.0], 1e-12));
        // x x^T + z z^T has its null space along y.
        let a = normalize([1.0, 1.0, 0.0]);
        let b = [0.0, 0.0, 1.0];
        let mut m = [[0.0; 3]; 3];
        for v in [a, b] {
            for i in 0..3 {
                for j in 0..3 {
                    m[i][j] += v[i] * v[j];
                }
            }
        }
        let e = smallest_eigenvector(m);
        let want = normalize([1.0, -1.0, 0.0]);
        assert!((dot(e, want).abs() - 1.0).abs() < 1e-9, "{e:?}");
    }

    /// Cameras with level x axes, world up = +Z: yaw about Z, then pitch the
    /// camera down about its own x axis. Camera axes: x right, y down, z forward.
    fn level_model(n: usize) -> Model {
        // cam_from_world for yaw = 0, pitch = 0 looks along world +Y with x = +X,
        // y = -Z: rows [1,0,0], [0,0,-1], [0,1,0], a rotation of +90° about X.
        let base = quat([1.0, 0.0, 0.0], std::f64::consts::FRAC_PI_2);
        let images = (0..n)
            .map(|k| {
                let yaw = quat([0.0, 0.0, 1.0], k as f64 * 0.7);
                let pitch = quat([1.0, 0.0, 0.0], 0.2 + 0.1 * k as f64);
                // cam_from_world = pitch_cam * base * yaw^-1
                let yaw_inv = [yaw[0], -yaw[1], -yaw[2], -yaw[3]];
                let q = qmul(pitch, qmul(base, yaw_inv));
                let image = Image {
                    rotation: q,
                    translation: [0.0; 3],
                    camera_id: 1,
                    name: format!("{k}.jpg"),
                    points2d: Vec::new(),
                };
                (k as u32 + 1, image)
            })
            .collect();
        Model {
            images,
            ..Model::default()
        }
    }

    #[test]
    fn camera_up_is_world_up_for_level_cameras() {
        let (up, tilt) = camera_up(&level_model(6)).unwrap();
        assert!(approx(up, [0.0, 0.0, 1.0], 1e-9), "{up:?}");
        assert!(tilt < 1e-6, "{tilt}");
    }

    #[test]
    fn waterline_pixels_are_kept_pixels_just_above_an_ignored_region() {
        // 1 column: rows 0-1 ignored (sky), 2-5 kept, 6-7 ignored (water).
        let mut m = image::GrayImage::new(1, 8);
        for y in 2..6 {
            m.put_pixel(0, y, image::Luma([255]));
        }
        let line = waterline_pixels(&m, 2);
        assert_eq!(line, [false, false, false, false, true, true, false, false]);
    }

    #[test]
    fn waterline_points_need_a_majority_of_waterline_observations() {
        // 10x10 masks: rows >= 6 ignored. Waterline band 2 => rows 4, 5.
        let tmp = tempfile::tempdir().unwrap();
        let mut mask = image::GrayImage::new(10, 10);
        for y in 0..6 {
            for x in 0..10 {
                mask.put_pixel(x, y, image::Luma([255]));
            }
        }
        let mut images = BTreeMap::new();
        for k in 1..=3u32 {
            mask.save(tmp.path().join(format!("{k}.jpg.png"))).unwrap();
            // point 1 on row 5 everywhere; point 2 on row 5 once, row 1 twice.
            let rows = [5.0, if k == 1 { 5.0 } else { 1.0 }];
            images.insert(
                k,
                Image {
                    rotation: [1.0, 0.0, 0.0, 0.0],
                    translation: [0.0; 3],
                    camera_id: 1,
                    name: format!("{k}.jpg"),
                    points2d: rows
                        .iter()
                        .enumerate()
                        .map(|(i, &y)| Point2D {
                            xy: [3.5, y + 0.5],
                            point3d: Some(i as u64 + 1),
                        })
                        .collect(),
                },
            );
        }
        let point = |id: u64, z: f64| {
            (
                id,
                Point3D {
                    xyz: [0.0, 0.0, z],
                    rgb: [0; 3],
                    error: None,
                    track: (1..=3).map(|k| (k, id as u32 - 1)).collect(),
                },
            )
        };
        let model = Model {
            images,
            points3d: [point(1, -1.0), point(2, 5.0)].into_iter().collect(),
            ..Model::default()
        };
        let pts = waterline_points(&model, tmp.path(), 2).unwrap();
        assert_eq!(pts, [[0.0, 0.0, -1.0]]);
    }

    #[test]
    fn lowest_mode_skips_a_higher_taller_cluster() {
        // Waterline cluster at 0 (40 points), deck edge at 1 (100 points), one outlier at -5.
        let mut v: Vec<f64> = (0..40).map(|i| (i as f64 - 20.0) * 0.001).collect();
        v.extend((0..100).map(|i| 1.0 + (i as f64 - 50.0) * 0.001));
        v.push(-5.0);
        let h = lowest_mode(&v, 0.02, 0.25).unwrap();
        assert!(h.abs() < 0.005, "{h}");
        assert_eq!(lowest_mode(&[], 0.02, 0.25), None);
    }

    #[test]
    fn frame_puts_the_plane_at_y_zero_with_its_normal_up() {
        let n = normalize([0.0, -0.9, -0.4]);
        let plane = Plane {
            normal: n,
            height: -1.5,
        };
        let f = Frame::on_plane(&plane, [3.0, 1.0, 2.0], [1.0, 0.0, 0.0]);
        // A point 0.25 above the plane is at local y = 0.25.
        let p = [5.0, 2.0, -1.0];
        let q = sub(p, scale(n, plane.signed_distance(p) - 0.25));
        assert!((f.to_local(q)[1] - 0.25).abs() < 1e-12);
        // Right-handed orthonormal rows.
        let [ex, ey, ez] = f.rows;
        assert!(approx(cross(ex, ey), ez, 1e-12));
        assert!(approx(f.to_local(f.origin), [0.0; 3], 1e-12));
        let m = f.matrix();
        let h = [0, 1, 2].map(|i| dot([m[i][0], m[i][1], m[i][2]], p) + m[i][3]);
        assert!(approx(h, f.to_local(p), 1e-12));
    }
}
