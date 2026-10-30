//! `post` stage: water plane, mesh cleanup and splat cleanup (ADR-0012).
//!
//! Fits the water plane from the `sfm` model and the `masks` (or takes a fixed
//! height from config), then:
//!
//! - mesh (`dense`'s `mesh_textured.glb`): moved into the model frame (`+Y`
//!   up, water at `y = 0`), faces below the water removed, components smaller
//!   than `post.min_component` removed, decimated to `post.target_faces`, the
//!   texture downscaled to `post.texture_max_size` and embedded as JPEG, and a
//!   water surface added as its own node: `model.glb`;
//! - splat (`splat`'s `splat.ply`): Gaussians below the water, unseen by any
//!   training view, or seen against masked water in more than
//!   `post.floater_max_water` of their views removed: `splat.ply`, still in
//!   the reconstruction frame (rotating SH coefficients is not worth it here);
//! - `water_plane.json`: the plane, how it was found, and the transform from
//!   the reconstruction frame to the model frame.

pub mod glb;
pub mod mesh;
pub mod plane;
pub mod splat;

use std::path::Path;

use serde::Serialize;
use shoreline_core::config::WaterPlane;
use shoreline_core::error::io_error;
use shoreline_core::stage::{Inputs, Metrics, StageRun};
use shoreline_core::{Config, Error, Result, RunContext, Stage};
use shoreline_sfm::Model;

use crate::glb::{Texture, TexturedMesh, Water};
use crate::plane::{Frame, Plane, Vec3};

pub const NAME: &str = "post";
const MASKS: &str = "masks";
const SFM: &str = "sfm";
const DENSE: &str = "dense";
const SPLAT: &str = "splat";

/// Bump when the output changes for the same config and inputs.
const REVISION: u32 = 2;

/// Lowest-mode peaks below this fraction of the highest are noise.
const MIN_PEAK: f64 = 0.25;
/// Kernel bandwidth as a fraction of the waterline points' spread.
const BANDWIDTH: f64 = 0.01;
/// Fewest waterline points for a fit.
const MIN_WATERLINE_POINTS: usize = 20;
/// Largest camera roll (degrees) for which the cameras' x axes give up.
pub const MAX_CAMERA_TILT_DEG: f64 = 5.0;
/// JPEG quality of the embedded texture.
const JPEG_QUALITY: u8 = 90;
/// The water surface extends this far beyond the mesh, relative to its extent.
const WATER_MARGIN: f32 = 2.0;
const WATER_COLOR: [f32; 4] = [0.11, 0.26, 0.32, 0.82];

/// Metric names.
pub const CAMERA_TILT_DEG: &str = "camera_tilt_deg";
pub const WATERLINE_POINTS: &str = "waterline_points";
pub const PLANE_INLIERS: &str = "plane_inliers";
pub const PLANE_RESIDUAL: &str = "plane_residual";
pub const MESH_FACES_IN: &str = "mesh_faces_in";
pub const MESH_FACES_BELOW_WATER: &str = "mesh_faces_below_water";
pub const MESH_COMPONENTS_REMOVED: &str = "mesh_components_removed";
pub const MESH_FACES: &str = "mesh_faces";
pub const MESH_VERTICES: &str = "mesh_vertices";
pub const DECIMATION_ERROR: &str = "decimation_error";
pub const VERTICES_BELOW_WATER: &str = "vertices_below_water";
pub const GAUSSIANS_IN: &str = "gaussians_in";
pub const GAUSSIANS_BELOW_WATER: &str = "gaussians_below_water";
pub const GAUSSIANS_UNSEEN: &str = "gaussians_unseen";
pub const GAUSSIANS_OVER_WATER: &str = "gaussians_over_water";
pub const GAUSSIANS: &str = "gaussians";
pub const CENTRES_BELOW_WATER: &str = "centres_below_water";

pub struct Post;

/// How the plane's height was found, with the fit's numbers.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "method", rename_all = "lowercase")]
pub enum Height {
    Fit {
        waterline_points: usize,
        inliers: usize,
        /// RMS distance of the inliers from the plane, in reconstruction units.
        residual: f64,
        bandwidth: f64,
    },
    Fixed,
}

/// `water_plane.json`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct WaterPlaneReport {
    pub plane: Plane,
    pub height: Height,
    pub camera_tilt_deg: f64,
    /// Row-major 4x4 transform from the reconstruction (COLMAP) frame to the
    /// frame of `model.glb`. `splat.ply` stays in the reconstruction frame.
    pub model_from_reconstruction: [[f64; 4]; 4],
}

/// Find the water plane: normal from the cameras, height fitted or fixed.
pub fn water_plane(
    model: &Model,
    masks: &Path,
    config: &shoreline_core::config::Post,
) -> Result<(Plane, Height, f64)> {
    let (up, tilt) = plane::camera_up(model)?;
    let points = match config.water_plane {
        WaterPlane::Height(h) => {
            return Ok((
                Plane {
                    normal: up,
                    height: h,
                },
                Height::Fixed,
                tilt,
            ));
        }
        WaterPlane::Fit(_) => plane::waterline_points(model, masks, config.waterline_band_px)?,
    };
    if points.len() < MIN_WATERLINE_POINTS {
        return Err(stage_error(format!(
            "{} waterline points (fewest for a fit: {MIN_WATERLINE_POINTS}); \
             set post.water_plane to a fixed height",
            points.len()
        )));
    }
    let heights: Vec<f64> = points.iter().map(|&p| plane::dot(up, p)).collect();
    let bandwidth = BANDWIDTH * plane::spread(&points);
    let h = plane::lowest_mode(&heights, bandwidth, MIN_PEAK)
        .ok_or_else(|| stage_error("no density peak among the waterline heights"))?;
    let inliers: Vec<f64> = heights
        .iter()
        .map(|v| v - h)
        .filter(|d| d.abs() <= 2.0 * bandwidth)
        .collect();
    let residual = (inliers.iter().map(|d| d * d).sum::<f64>() / inliers.len() as f64).sqrt();
    let fit = Height::Fit {
        waterline_points: points.len(),
        inliers: inliers.len(),
        residual,
        bandwidth,
    };
    Ok((
        Plane {
            normal: up,
            height: h,
        },
        fit,
        tilt,
    ))
}

/// The model frame: origin under the mean camera centre, X along the mean
/// camera x axis.
pub fn model_frame(model: &Model, plane: &Plane) -> Frame {
    let n = model.images.len() as f64;
    let mut centre = [0.0; 3];
    let mut right = [0.0; 3];
    for image in model.images.values() {
        let r = plane::rotation(image.rotation);
        let t = image.translation;
        for k in 0..3 {
            centre[k] -= (r[0][k] * t[0] + r[1][k] * t[1] + r[2][k] * t[2]) / n;
            right[k] += r[0][k] / n;
        }
    }
    Frame::on_plane(plane, centre, right)
}

/// Decode, downscale to `max_size` on the longest edge and encode as JPEG.
pub fn web_texture(texture: &Texture, max_size: u32) -> Result<Texture> {
    let img = image::load_from_memory(&texture.bytes)
        .map_err(|e| stage_error(format!("texture: {e}")))?
        .into_rgb8();
    let img = if img.width().max(img.height()) > max_size {
        image::DynamicImage::ImageRgb8(img)
            .resize(max_size, max_size, image::imageops::FilterType::Lanczos3)
            .into_rgb8()
    } else {
        img
    };
    let mut bytes = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut bytes, JPEG_QUALITY)
        .encode_image(&img)
        .map_err(|e| stage_error(format!("texture: {e}")))?;
    Ok(Texture {
        bytes,
        mime: "image/jpeg".into(),
    })
}

/// A rectangle at `y = 0` around the mesh's XZ extent plus a margin,
/// counter-clockwise seen from above.
pub fn water_outline(positions: &[[f32; 3]]) -> Vec<[f32; 3]> {
    let (mut lo, mut hi) = ([f32::INFINITY; 2], [f32::NEG_INFINITY; 2]);
    for p in positions {
        for (k, c) in [p[0], p[2]].into_iter().enumerate() {
            lo[k] = lo[k].min(c);
            hi[k] = hi[k].max(c);
        }
    }
    let margin = WATER_MARGIN * (hi[0] - lo[0]).max(hi[1] - lo[1]);
    let (x0, x1, z0, z1) = (
        lo[0] - margin,
        hi[0] + margin,
        lo[1] - margin,
        hi[1] + margin,
    );
    // Seen from +Y looking down, X right and Z down the page: (x0,z1) -> (x1,z1) -> (x1,z0) -> (x0,z0).
    vec![[x0, 0.0, z1], [x1, 0.0, z1], [x1, 0.0, z0], [x0, 0.0, z0]]
}

impl Stage for Post {
    fn name(&self) -> &'static str {
        NAME
    }

    fn deps(&self) -> &[&'static str] {
        &[MASKS, SFM, DENSE, SPLAT]
    }

    fn config(&self, config: &Config) -> serde_json::Value {
        serde_json::to_value(&config.post).expect("post config serialises to JSON")
    }

    fn version(&self, _ctx: &RunContext) -> Result<String> {
        Ok(format!("{NAME}/{REVISION}"))
    }

    fn run(
        &self,
        ctx: &RunContext,
        inputs: &Inputs,
        out_dir: &Path,
        _on_event: &mut dyn FnMut(shoreline_core::pipeline::Event<'_>),
    ) -> Result<StageRun> {
        let config = &ctx.config.post;
        let model = Model::read_binary(&inputs[SFM].dir.join("sparse/0"))?;
        let (plane, height, tilt) = water_plane(&model, &inputs[MASKS].dir.join("masks"), config)?;
        let frame = model_frame(&model, &plane);
        let mut metrics = Metrics::new();
        let mut metric = |k: &str, v: f64| {
            metrics.insert(k.to_owned(), v);
        };
        metric(CAMERA_TILT_DEG, tilt);
        if let Height::Fit {
            waterline_points,
            inliers,
            residual,
            ..
        } = height
        {
            metric(WATERLINE_POINTS, waterline_points as f64);
            metric(PLANE_INLIERS, inliers as f64);
            metric(PLANE_RESIDUAL, residual);
        }

        // Mesh.
        let input = glb::read(&inputs[DENSE].dir.join("mesh_textured.glb"))?;
        metric(MESH_FACES_IN, input.mesh.faces() as f64);
        let local = input
            .mesh
            .map_positions(|p| frame.to_local(p.map(f64::from)).map(|x| x as f32));
        let clipped = local.clip_below(0.0);
        metric(
            MESH_FACES_BELOW_WATER,
            (local.faces() - clipped.faces()) as f64,
        );
        let (cleaned, removed) = clipped.remove_small_components(config.min_component);
        metric(MESH_COMPONENTS_REMOVED, removed as f64);
        let (mesh, error) = cleaned.decimate(config.target_faces as usize);
        metric(DECIMATION_ERROR, f64::from(error));
        metric(MESH_FACES, mesh.faces() as f64);
        metric(MESH_VERTICES, mesh.positions.len() as f64);
        metric(VERTICES_BELOW_WATER, mesh.vertices_below(0.0) as f64);
        let water = Water {
            outline: water_outline(&mesh.positions),
            color: WATER_COLOR,
        };
        let texture = web_texture(&input.texture, config.texture_max_size)?;
        glb::write(
            &out_dir.join("model.glb"),
            &TexturedMesh { mesh, texture },
            Some(&water),
        )?;

        // Splat.
        let splat_dir = &inputs[SPLAT].dir;
        let splat = splat::Splat::read(&splat_dir.join("splat.ply"))?;
        let centres = splat.centres();
        let cameras = Model::read_binary(&splat_dir.join("dataset/sparse/0"))?;
        let (seen, water_views) =
            splat::view_counts(&centres, &cameras, &splat_dir.join("dataset/masks"), &plane)?;
        let (keep, counts) = splat::cull(
            &centres,
            &seen,
            &water_views,
            &plane,
            config.floater_max_water,
        );
        let kept = splat.retain(&keep);
        kept.write(&out_dir.join("splat.ply"))?;
        metric(GAUSSIANS_IN, centres.len() as f64);
        metric(GAUSSIANS_BELOW_WATER, counts.below_water as f64);
        metric(GAUSSIANS_UNSEEN, counts.unseen as f64);
        metric(GAUSSIANS_OVER_WATER, counts.over_water as f64);
        metric(GAUSSIANS, counts.kept as f64);
        let below = kept
            .centres()
            .iter()
            .filter(|&&p: &&Vec3| plane.signed_distance(p) < 0.0)
            .count();
        metric(CENTRES_BELOW_WATER, below as f64);

        let report = WaterPlaneReport {
            plane,
            height,
            camera_tilt_deg: tilt,
            model_from_reconstruction: frame.matrix(),
        };
        let path = out_dir.join("water_plane.json");
        let json = serde_json::to_string_pretty(&report).expect("report serialises");
        std::fs::write(&path, json + "\n").map_err(io_error(&path))?;
        Ok(StageRun {
            tools: Vec::new(),
            metrics,
        })
    }

    fn gate(&self, config: &Config, metrics: &Metrics) -> Result<Vec<String>> {
        let get = |k: &str| metrics.get(k).copied().unwrap_or(0.0);
        let mut failures = Vec::new();
        if get(VERTICES_BELOW_WATER) > 0.0 || get(CENTRES_BELOW_WATER) > 0.0 {
            failures.push(format!(
                "geometry below the water plane after post: {} mesh vertices, {} splat centres (want 0)",
                get(VERTICES_BELOW_WATER),
                get(CENTRES_BELOW_WATER)
            ));
        }
        if get(CAMERA_TILT_DEG) > MAX_CAMERA_TILT_DEG {
            failures.push(format!(
                "camera x axes are up to {:.1}° from level (at most {MAX_CAMERA_TILT_DEG}°), \
                 so the cameras do not give the water plane's normal",
                get(CAMERA_TILT_DEG)
            ));
        }
        if failures.is_empty() {
            let target = f64::from(config.post.target_faces);
            let warnings = (get(MESH_FACES) > target)
                .then(|| {
                    format!(
                        "mesh has {} faces, over post.target_faces {target}: texture seams stopped the decimation",
                        get(MESH_FACES)
                    )
                })
                .into_iter()
                .collect();
            Ok(warnings)
        } else {
            Err(stage_error(failures.join("; ")))
        }
    }
}

fn stage_error(message: impl Into<String>) -> Error {
    Error::Stage {
        stage: NAME,
        message: message.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn water_outline_surrounds_the_mesh_with_a_margin() {
        let o = water_outline(&[[0.0, 1.0, 0.0], [2.0, 3.0, 1.0]]);
        assert_eq!(
            o,
            [
                [-4.0, 0.0, 5.0],
                [6.0, 0.0, 5.0],
                [6.0, 0.0, -4.0],
                [-4.0, 0.0, -4.0]
            ]
        );
        // Counter-clockwise seen from +Y: the fan's first triangle faces up.
        let (a, b, c) = (o[0], o[1], o[2]);
        let u = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
        let v = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
        assert!(u[2] * v[0] - u[0] * v[2] > 0.0, "normal has +Y");
    }

    #[test]
    fn web_texture_downscales_to_jpeg() {
        let img = image::RgbImage::from_pixel(64, 32, image::Rgb([200, 100, 50]));
        let mut png = Vec::new();
        image::DynamicImage::ImageRgb8(img)
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .unwrap();
        let t = web_texture(
            &Texture {
                bytes: png,
                mime: "image/png".into(),
            },
            16,
        )
        .unwrap();
        assert_eq!(t.mime, "image/jpeg");
        let back = image::load_from_memory(&t.bytes).unwrap();
        assert_eq!((back.width(), back.height()), (16, 8));
    }

    #[test]
    fn gate_fails_on_geometry_below_water_and_on_tilted_cameras() {
        let config = Config::load(
            &Path::new(env!("CARGO_MANIFEST_DIR")).join("../../configs/reposalmi-laajasalo.toml"),
        )
        .unwrap();
        let metrics = |pairs: &[(&str, f64)]| -> Metrics {
            pairs.iter().map(|&(k, v)| (k.to_owned(), v)).collect()
        };
        assert!(
            Post.gate(&config, &metrics(&[(CAMERA_TILT_DEG, 0.9)]))
                .unwrap()
                .is_empty()
        );
        let err = Post
            .gate(&config, &metrics(&[(VERTICES_BELOW_WATER, 3.0)]))
            .unwrap_err()
            .to_string();
        assert!(err.contains("3 mesh vertices"), "{err}");
        let err = Post
            .gate(&config, &metrics(&[(CAMERA_TILT_DEG, 12.0)]))
            .unwrap_err()
            .to_string();
        assert!(err.contains("12.0°"), "{err}");
        let warnings = Post
            .gate(&config, &metrics(&[(MESH_FACES, 743_787.0)]))
            .unwrap();
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("743787 faces"), "{warnings:?}");
    }
}
