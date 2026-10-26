//! `video` stage: the showcase video of the splat (ADR-0008, #29).
//!
//! Runs the render image's `render_video` on the splat and its training
//! cameras (the splat stage's undistorted `dataset/sparse/0`), with a sidebar
//! built from the sfm and splat manifests. Output: `showcase.mp4`,
//! `showcase-share.mp4` (at most `video.share_max_mib`), `meta.json` and
//! `logs/render_video.log`.

use std::ffi::OsString;
use std::path::Path;

use serde_json::{Value, json};
use shoreline_core::cache;
use shoreline_core::config::{CameraPath, Video};
use shoreline_core::error::io_error;
use shoreline_core::stage::{Inputs, Metrics, StageOutput, StageRun};
use shoreline_core::tools::Invocation;
use shoreline_core::{Config, Error, Result, RunContext, Stage};
use shoreline_sfm::metrics as sfm;

pub const NAME: &str = "video";
const SFM: &str = "sfm";
const SPLAT: &str = "splat";
const RENDER: &str = "render";

/// Bump when the output changes for the same config, inputs and tool.
const REVISION: u32 = 1;

/// Metric names.
pub const VIDEO_MIB: &str = "video_mib";
pub const SHARE_MIB: &str = "share_mib";

pub struct VideoStage;

fn path_name(path: CameraPath) -> &'static str {
    match path {
        CameraPath::Arc => "arc",
        CameraPath::Drone => "drone",
    }
}

/// `render_video` arguments.
pub fn render_args(
    video: &Video,
    splat: &Path,
    sparse: &Path,
    meta: &Path,
    out: &Path,
) -> Vec<OsString> {
    let mut args: Vec<OsString> = vec![
        splat.join("splat.ply").into(),
        sparse.into(),
        meta.into(),
        out.join("showcase.mp4").into(),
    ];
    let opts = [
        ("--path", path_name(video.path).to_owned()),
        ("--seconds", video.seconds.to_string()),
        ("--fps", video.fps.to_string()),
        ("--size", format!("{}x{}", video.width, video.height)),
        ("--share-max-mib", video.share_max_mib.to_string()),
    ];
    for (flag, value) in opts {
        args.extend([OsString::from(flag), value.into()]);
    }
    args.extend([
        OsString::from("--share"),
        out.join("showcase-share.mp4").into(),
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

/// The sidebar (`render_video`'s meta.json) from the sfm and splat metrics.
/// Rows with a gate carry its pass/fail state.
pub fn meta(config: &Config, sfm_metrics: &Metrics, splat_metrics: &Metrics) -> Value {
    let g = &config.gates;
    let m = |k: &str| sfm_metrics.get(k).copied().unwrap_or(f64::NAN);
    let ratio = m(sfm::REGISTERED_RATIO);
    let reproj = m(sfm::MEAN_REPROJ_ERROR_PX);
    let track = m(sfm::MEAN_TRACK_LENGTH);
    let gaussians = splat_metrics.get("gaussians").copied().unwrap_or(f64::NAN);
    json!({
        "title": config.dataset.name,
        "subtitle": "shoreline-recon showcase",
        "kind": "masked",
        "sections": [
            ["SfM", [
                ["Registered", format!("{} / {}", m(sfm::REGISTERED_IMAGES), m(sfm::INPUT_IMAGES)), ratio >= g.min_registered_ratio],
                ["Reprojection", format!("{reproj:.2} px"), reproj <= g.max_reproj_error_px],
                ["Track length", format!("{track:.2}"), track >= g.min_track_length],
                ["Points", thousands(m(sfm::POINTS))],
            ]],
            ["Splat", [
                ["Gaussians", thousands(gaussians)],
                ["Steps", thousands(f64::from(config.splat.steps))],
            ]],
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
        &[SFM, SPLAT]
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
        let sparse = splat.join("dataset/sparse/0");
        let meta_path = out_dir.join("meta.json");
        let meta = meta(
            &ctx.config,
            &manifest_metrics(SFM, &inputs[SFM])?,
            &manifest_metrics(SPLAT, &inputs[SPLAT])?,
        );
        let text = serde_json::to_string_pretty(&meta).expect("meta serialises");
        std::fs::write(&meta_path, text).map_err(io_error(&meta_path))?;

        let args = render_args(&ctx.config.video, splat, &sparse, &meta_path, out_dir);
        let inv = Invocation::new(args).mount(splat).mount(out_dir);
        let record = ctx
            .tools
            .get(RENDER)?
            .run(&inv, &out_dir.join("logs/render_video.log"))?;

        let metrics = [
            (VIDEO_MIB, mib(&out_dir.join("showcase.mp4"))?),
            (SHARE_MIB, mib(&out_dir.join("showcase-share.mp4"))?),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_owned(), v))
        .collect();
        Ok(StageRun {
            tools: vec![record],
            metrics,
        })
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
        let args = render_args(
            &config.video,
            Path::new("/s"),
            Path::new("/s/dataset/sparse/0"),
            Path::new("/o/meta.json"),
            Path::new("/o"),
        );
        assert_eq!(
            strings(&args),
            [
                "/s/splat.ply",
                "/s/dataset/sparse/0",
                "/o/meta.json",
                "/o/showcase.mp4",
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
                "/o/showcase-share.mp4",
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
        let splat_metrics: Metrics = [("gaussians".to_owned(), 595317.0)].into();
        let meta = meta(&config, &sfm_metrics, &splat_metrics);
        assert_eq!(meta["title"], "reposalmi-laajasalo");
        let sfm_rows = &meta["sections"][0][1];
        assert_eq!(sfm_rows[0], json!(["Registered", "121 / 121", true]));
        assert_eq!(sfm_rows[1], json!(["Reprojection", "1.50 px", false]));
        assert_eq!(sfm_rows[3], json!(["Points", "68,582"]));
        assert_eq!(meta["sections"][1][1][0], json!(["Gaussians", "595,317"]));
    }

    #[test]
    fn thousands_groups_digits() {
        assert_eq!(thousands(5.0), "5");
        assert_eq!(thousands(1234.0), "1,234");
        assert_eq!(thousands(1234567.0), "1,234,567");
    }
}
