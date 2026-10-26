//! `video` stage: the showcase videos of the splat and the mesh (ADR-0008,
//! #29).
//!
//! Runs the render image's `render_video` for every scene and every view in
//! `video.views`. The camera paths are derived from the splat's training
//! cameras (the splat stage's undistorted `dataset/sparse/0`). The scenes are
//! the post stage's cleaned `splat.ply`, and its `model.glb` with the water
//! surface (moved into the model frame by `water_plane.json`, ADR-0012). The
//! sidebars come from the sfm and post manifests. Output:
//! `<scene>-<path>.mp4` (`splat-arc.mp4`, `mesh-drone.mp4`, ...), each with a
//! `-share` copy (at most `video.share_max_mib`), `meta.json`,
//! `mesh-meta.json` and `logs/render_<scene>-<path>.log`.

use std::ffi::OsString;
use std::path::Path;

use serde_json::{Value, json};
use shoreline_core::cache;
use shoreline_core::config::{CameraPath, Video, View};
use shoreline_core::error::io_error;
use shoreline_core::stage::{Inputs, Metrics, StageOutput, StageRun};
use shoreline_core::tools::Invocation;
use shoreline_core::{Config, Error, Result, RunContext, Stage};
use shoreline_post as post;
use shoreline_sfm::metrics as sfm;

pub const NAME: &str = "video";
const SFM: &str = "sfm";
const SPLAT: &str = "splat";
const POST: &str = "post";
const RENDER: &str = "render";

/// Bump when the output changes for the same config, inputs and tool.
const REVISION: u32 = 3;

/// The rendered scenes, in render order.
pub const SCENES: &[&str] = &["splat", "mesh"];

pub struct VideoStage;

pub fn path_name(path: CameraPath) -> &'static str {
    match path {
        CameraPath::Arc => "arc",
        CameraPath::Drone => "drone",
    }
}

/// `<scene>-<path>`, the stem of one rendered video.
pub fn video_name(scene: &str, view: &View) -> String {
    format!("{scene}-{}", path_name(view.path))
}

/// Every file name the stage writes for `video`, in scene then view order:
/// each video followed by its share copy.
pub fn video_files(video: &Video) -> Vec<String> {
    SCENES
        .iter()
        .flat_map(|scene| video.views.iter().map(move |v| video_name(scene, v)))
        .flat_map(|name| [format!("{name}.mp4"), format!("{name}-share.mp4")])
        .collect()
}

/// `render_video` arguments: `scene` (a splat `.ply`, or a `.glb` with its
/// `frame`) along `view`, rendered to `<out>/<name>.mp4` and
/// `<out>/<name>-share.mp4`.
#[allow(clippy::too_many_arguments)]
pub fn render_args(
    video: &Video,
    view: &View,
    scene: &Path,
    frame: Option<&Path>,
    sparse: &Path,
    meta: &Path,
    out: &Path,
    name: &str,
) -> Vec<OsString> {
    let mut args: Vec<OsString> = vec![
        scene.into(),
        sparse.into(),
        meta.into(),
        out.join(format!("{name}.mp4")).into(),
    ];
    if let Some(frame) = frame {
        args.extend([OsString::from("--frame"), frame.into()]);
    }
    let opts = [
        ("--path", path_name(view.path).to_owned()),
        ("--seconds", view.seconds.to_string()),
        ("--fps", video.fps.to_string()),
        ("--size", format!("{}x{}", video.width, video.height)),
        ("--share-max-mib", video.share_max_mib.to_string()),
    ];
    for (flag, value) in opts {
        args.extend([OsString::from(flag), value.into()]);
    }
    args.extend([
        OsString::from("--share"),
        out.join(format!("{name}-share.mp4")).into(),
    ]);
    args
}

/// `1234567` → `1,234,567`.
fn thousands(n: f64) -> String {
    let digits = format!("{}", n.round() as i64);
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// The SfM sidebar section. Rows with a gate carry its pass/fail state.
fn sfm_section(config: &Config, sfm_metrics: &Metrics) -> Value {
    let g = &config.gates;
    let m = |k: &str| sfm_metrics.get(k).copied().unwrap_or(f64::NAN);
    let ratio = m(sfm::REGISTERED_RATIO);
    let reproj = m(sfm::MEAN_REPROJ_ERROR_PX);
    let track = m(sfm::MEAN_TRACK_LENGTH);
    json!([
        "SfM",
        [
            [
                "Registered",
                format!("{} / {}", m(sfm::REGISTERED_IMAGES), m(sfm::INPUT_IMAGES)),
                ratio >= g.min_registered_ratio
            ],
            [
                "Reprojection",
                format!("{reproj:.2} px"),
                reproj <= g.max_reproj_error_px
            ],
            [
                "Track length",
                format!("{track:.2}"),
                track >= g.min_track_length
            ],
            ["Points", thousands(m(sfm::POINTS))],
        ]
    ])
}

/// The splat video's sidebar (`render_video`'s meta.json) from the sfm and
/// post metrics.
pub fn meta(config: &Config, sfm_metrics: &Metrics, post_metrics: &Metrics) -> Value {
    let p = |k: &str| post_metrics.get(k).copied().unwrap_or(f64::NAN);
    let removed = p(post::GAUSSIANS_IN) - p(post::GAUSSIANS);
    json!({
        "title": config.dataset.name,
        "subtitle": "shoreline-recon · Gaussian splat",
        "kind": "masked",
        "sections": [
            sfm_section(config, sfm_metrics),
            ["Splat", [
                ["Gaussians", thousands(p(post::GAUSSIANS))],
                ["Floaters removed", thousands(removed)],
                ["Steps", thousands(f64::from(config.splat.steps))],
            ]],
        ],
    })
}

/// The mesh video's sidebar from the sfm and post metrics.
pub fn mesh_meta(config: &Config, sfm_metrics: &Metrics, post_metrics: &Metrics) -> Value {
    let p = |k: &str| post_metrics.get(k).copied().unwrap_or(f64::NAN);
    let mut water = vec![json!([
        "Below water",
        thousands(p(post::VERTICES_BELOW_WATER)),
        p(post::VERTICES_BELOW_WATER) == 0.0
    ])];
    if post_metrics.contains_key(post::PLANE_INLIERS) {
        water.insert(
            0,
            json!([
                "Waterline fit",
                format!(
                    "{} / {}",
                    thousands(p(post::PLANE_INLIERS)),
                    thousands(p(post::WATERLINE_POINTS))
                )
            ]),
        );
    }
    json!({
        "title": config.dataset.name,
        "subtitle": "shoreline-recon · textured mesh",
        "kind": "masked",
        "sections": [
            sfm_section(config, sfm_metrics),
            ["Mesh", [
                ["Faces", thousands(p(post::MESH_FACES))],
                ["From", thousands(p(post::MESH_FACES_IN))],
            ]],
            ["Water plane", water],
        ],
    })
}

fn manifest_metrics(name: &str, input: &StageOutput) -> Result<Metrics> {
    cache::lookup(&input.dir, &input.fingerprint)?
        .map(|m| m.metrics)
        .ok_or_else(|| stage_error(format!("no manifest for {name} in {}", input.dir.display())))
}

fn mib(path: &Path) -> Result<f64> {
    let len = std::fs::metadata(path).map_err(io_error(path))?.len();
    Ok(len as f64 / (1024.0 * 1024.0))
}

impl Stage for VideoStage {
    fn name(&self) -> &'static str {
        NAME
    }

    fn deps(&self) -> &[&'static str] {
        &[SFM, SPLAT, POST]
    }

    fn config(&self, config: &Config) -> serde_json::Value {
        serde_json::to_value(&config.video).expect("video config serialises to JSON")
    }

    fn version(&self, ctx: &RunContext) -> Result<String> {
        let render = ctx.tools.get(RENDER)?;
        if render.spec.image.is_none() {
            return Err(stage_error(
                "the render tool must run from its pinned image (ADR-0011)",
            ));
        }
        Ok(format!("{NAME}/{REVISION} {}", render.version(&[])?))
    }

    fn run(&self, ctx: &RunContext, inputs: &Inputs, out_dir: &Path) -> Result<StageRun> {
        let splat = &inputs[SPLAT].dir;
        let post_dir = &inputs[POST].dir;
        let sparse = splat.join("dataset/sparse/0");
        let sfm_metrics = manifest_metrics(SFM, &inputs[SFM])?;
        let post_metrics = manifest_metrics(POST, &inputs[POST])?;
        let write_meta = |name: &str, value: &Value| -> Result<std::path::PathBuf> {
            let path = out_dir.join(name);
            let text = serde_json::to_string_pretty(value).expect("meta serialises");
            std::fs::write(&path, text).map_err(io_error(&path))?;
            Ok(path)
        };
        let splat_meta = write_meta("meta.json", &meta(&ctx.config, &sfm_metrics, &post_metrics))?;
        let mesh_meta = write_meta(
            "mesh-meta.json",
            &mesh_meta(&ctx.config, &sfm_metrics, &post_metrics),
        )?;

        let render = ctx.tools.get(RENDER)?;
        let video = &ctx.config.video;
        let mut tools = Vec::new();
        let mut metrics = Metrics::new();
        for &scene in SCENES {
            let (file, frame, meta_path) = match scene {
                "splat" => (post_dir.join("splat.ply"), None, &splat_meta),
                _ => (
                    post_dir.join("model.glb"),
                    Some(post_dir.join("water_plane.json")),
                    &mesh_meta,
                ),
            };
            for view in &video.views {
                let name = video_name(scene, view);
                let args = render_args(
                    video,
                    view,
                    &file,
                    frame.as_deref(),
                    &sparse,
                    meta_path,
                    out_dir,
                    &name,
                );
                let inv = Invocation::new(args)
                    .mount(splat)
                    .mount(post_dir)
                    .mount(out_dir);
                let log = out_dir.join(format!("logs/render_{name}.log"));
                tools.push(render.run(&inv, &log)?);
                let size = |f: String| mib(&out_dir.join(f));
                metrics.insert(format!("{name}_mib"), size(format!("{name}.mp4"))?);
                metrics.insert(
                    format!("{name}_share_mib"),
                    size(format!("{name}-share.mp4"))?,
                );
            }
        }
        Ok(StageRun { tools, metrics })
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

    fn repo_config() -> Config {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../configs");
        Config::load(&dir.join("reposalmi-laajasalo.toml")).unwrap()
    }

    fn strings(args: &[OsString]) -> Vec<String> {
        args.iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn render_args_follow_the_video_config() {
        let config = repo_config();
        let arc = config.video.views[0];
        let drone = config.video.views[1];
        let args = render_args(
            &config.video,
            &arc,
            Path::new("/p/splat.ply"),
            None,
            Path::new("/s/dataset/sparse/0"),
            Path::new("/o/meta.json"),
            Path::new("/o"),
            &video_name("splat", &arc),
        );
        assert_eq!(
            strings(&args),
            [
                "/p/splat.ply",
                "/s/dataset/sparse/0",
                "/o/meta.json",
                "/o/splat-arc.mp4",
                "--path",
                "arc",
                "--seconds",
                "16",
                "--fps",
                "30",
                "--size",
                "1920x1080",
                "--share-max-mib",
                "30",
                "--share",
                "/o/splat-arc-share.mp4",
            ]
        );
        let mesh = strings(&render_args(
            &config.video,
            &drone,
            Path::new("/p/model.glb"),
            Some(Path::new("/p/water_plane.json")),
            Path::new("/s/dataset/sparse/0"),
            Path::new("/o/mesh-meta.json"),
            Path::new("/o"),
            &video_name("mesh", &drone),
        ));
        assert_eq!(
            mesh[..6],
            [
                "/p/model.glb",
                "/s/dataset/sparse/0",
                "/o/mesh-meta.json",
                "/o/mesh-drone.mp4",
                "--frame",
                "/p/water_plane.json"
            ]
        );
        assert_eq!(mesh[6..10], ["--path", "drone", "--seconds", "20"]);
        assert_eq!(mesh.last().unwrap(), "/o/mesh-drone-share.mp4");
    }

    #[test]
    fn video_files_cover_every_scene_and_view() {
        assert_eq!(
            video_files(&repo_config().video),
            [
                "splat-arc.mp4",
                "splat-arc-share.mp4",
                "splat-drone.mp4",
                "splat-drone-share.mp4",
                "mesh-arc.mp4",
                "mesh-arc-share.mp4",
                "mesh-drone.mp4",
                "mesh-drone-share.mp4",
            ]
        );
    }

    #[test]
    fn meta_shows_gate_states_and_counts() {
        let config = repo_config();
        let sfm_metrics: Metrics = [
            (sfm::REGISTERED_IMAGES, 121.0),
            (sfm::INPUT_IMAGES, 121.0),
            (sfm::REGISTERED_RATIO, 1.0),
            (sfm::MEAN_REPROJ_ERROR_PX, 1.5),
            (sfm::MEAN_TRACK_LENGTH, 7.12),
            (sfm::POINTS, 68582.0),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_owned(), v))
        .collect();
        let post_metrics: Metrics = [
            (post::GAUSSIANS_IN, 595136.0),
            (post::GAUSSIANS, 579344.0),
            (post::MESH_FACES, 500000.0),
            (post::MESH_FACES_IN, 1959305.0),
            (post::VERTICES_BELOW_WATER, 0.0),
            (post::PLANE_INLIERS, 803.0),
            (post::WATERLINE_POINTS, 2112.0),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_owned(), v))
        .collect();
        let meta = meta(&config, &sfm_metrics, &post_metrics);
        assert_eq!(meta["title"], "reposalmi-laajasalo");
        let sfm_rows = &meta["sections"][0][1];
        assert_eq!(sfm_rows[0], json!(["Registered", "121 / 121", true]));
        assert_eq!(sfm_rows[1], json!(["Reprojection", "1.50 px", false]));
        assert_eq!(sfm_rows[3], json!(["Points", "68,582"]));
        assert_eq!(meta["sections"][1][1][0], json!(["Gaussians", "579,344"]));
        assert_eq!(
            meta["sections"][1][1][1],
            json!(["Floaters removed", "15,792"])
        );

        let mesh = mesh_meta(&config, &sfm_metrics, &post_metrics);
        assert_eq!(mesh["sections"][1][1][0], json!(["Faces", "500,000"]));
        assert_eq!(
            mesh["sections"][2][1][0],
            json!(["Waterline fit", "803 / 2,112"])
        );
        assert_eq!(mesh["sections"][2][1][1], json!(["Below water", "0", true]));
    }

    #[test]
    fn thousands_groups_digits() {
        assert_eq!(thousands(5.0), "5");
        assert_eq!(thousands(1234.0), "1,234");
        assert_eq!(thousands(1234567.0), "1,234,567");
    }
}
