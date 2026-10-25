//! `frames` stage: frames from a video at `frames.fps`, the long edge capped at
//! `frames.max_long_edge`, and frames blurrier than `frames.blur_threshold`
//! moved aside (ADR-0005). Photo-directory input is not implemented yet (#23);
//! near-duplicate dropping is #10.
//!
//! Output: `images/00001.jpg…` (kept), `rejected/` (too blurry),
//! `frames.csv` (every frame's score), `logs/ffmpeg.log`.

pub mod blur;

use std::fmt::Write as _;
use std::path::Path;

use shoreline_core::config::Frames as FramesConfig;
use shoreline_core::error::io_error;
use shoreline_core::stage::{Inputs, Metrics, StageRun};
use shoreline_core::tools::Invocation;
use shoreline_core::{Config, Error, Result, RunContext, Stage};

pub const NAME: &str = "frames";

/// Bump when the output changes for the same config, input and ffmpeg.
const REVISION: u32 = 1;

pub struct Frames;

/// One extracted frame's blur score.
#[derive(Debug, Clone, PartialEq)]
pub struct Score {
    pub image: String,
    pub laplacian_var: f64,
    pub kept: bool,
}

impl Stage for Frames {
    fn name(&self) -> &'static str {
        NAME
    }

    fn deps(&self) -> &[&'static str] {
        &[]
    }

    fn config(&self, config: &Config) -> serde_json::Value {
        serde_json::to_value(&config.frames).expect("frames config serialises to JSON")
    }

    fn version(&self, ctx: &RunContext) -> Result<String> {
        let ffmpeg = ctx.tools.get("ffmpeg")?.version(&["-version"])?;
        Ok(format!("{NAME}/{REVISION} {ffmpeg}"))
    }

    fn run(&self, ctx: &RunContext, _inputs: &Inputs, out_dir: &Path) -> Result<StageRun> {
        let input = &ctx.config.dataset.input;
        if input.is_dir() {
            return Err(stage_error(format!(
                "{} is a directory; photo-directory input is not implemented yet (#23)",
                input.display()
            )));
        }
        let input = std::path::absolute(input).map_err(io_error(input))?;
        let images = out_dir.join("images");
        std::fs::create_dir_all(&images).map_err(io_error(&images))?;

        let extract = extract_invocation(&input, &images, &ctx.config.frames);
        let record = ctx
            .tools
            .get("ffmpeg")?
            .run(&extract, &out_dir.join("logs/ffmpeg.log"))?;

        let threshold = ctx.config.frames.blur_threshold;
        let scores = select(&images, &out_dir.join("rejected"), threshold)?;
        let csv = out_dir.join("frames.csv");
        std::fs::write(&csv, scores_csv(&scores)).map_err(io_error(&csv))?;
        if !scores.iter().any(|s| s.kept) {
            return Err(stage_error(format!(
                "no frame reached blur_threshold = {threshold}; scores are in {}",
                csv.display()
            )));
        }
        Ok(StageRun {
            tools: vec![record],
            metrics: metrics(&scores),
        })
    }
}

/// `fps=…[,scale=…]`: the long edge is capped, never enlarged, and the
/// short edge keeps the aspect ratio at an even size.
pub fn video_filter(frames: &FramesConfig) -> String {
    let fps = format!("fps={}", frames.fps);
    match frames.max_long_edge {
        Some(l) => format!(
            "{fps},scale='if(gte(iw,ih),min({l},iw),-2)':'if(gte(iw,ih),-2,min({l},ih))':flags=lanczos"
        ),
        None => fps,
    }
}

/// ffmpeg extracting `input` into `images/%05d.jpg`, numbered from 1.
pub fn extract_invocation(input: &Path, images: &Path, frames: &FramesConfig) -> Invocation {
    let mut inv = Invocation::new(["-v", "error", "-nostdin", "-i"]);
    inv.args.push(input.as_os_str().into());
    inv.args.push("-vf".into());
    inv.args.push(video_filter(frames).into());
    inv.args
        .extend(["-q:v", "2", "-start_number", "1"].map(Into::into));
    inv.args.push(images.join("%05d.jpg").into_os_string());
    let inv = match input.parent() {
        Some(dir) => inv.mount(dir),
        None => inv,
    };
    inv.mount(images)
}

/// Score every `*.jpg` in `images` (sorted by name) and move those below
/// `threshold` into `rejected`.
pub fn select(images: &Path, rejected: &Path, threshold: f64) -> Result<Vec<Score>> {
    let mut names = Vec::new();
    for entry in std::fs::read_dir(images).map_err(io_error(images))? {
        let name = entry.map_err(io_error(images))?.file_name();
        if let Some(name) = name.to_str().filter(|n| n.ends_with(".jpg")) {
            names.push(name.to_owned());
        }
    }
    names.sort();
    if names.is_empty() {
        return Err(stage_error(format!("no frames in {}", images.display())));
    }
    let mut scores = Vec::with_capacity(names.len());
    for image in names {
        let path = images.join(&image);
        let laplacian_var = blur::laplacian_variance(&blur::decode_luma(&path)?);
        let kept = laplacian_var >= threshold;
        if !kept {
            std::fs::create_dir_all(rejected).map_err(io_error(rejected))?;
            std::fs::rename(&path, rejected.join(&image)).map_err(io_error(&path))?;
        }
        scores.push(Score {
            image,
            laplacian_var,
            kept,
        });
    }
    Ok(scores)
}

/// `image,laplacian_var,kept` rows, as the M0 spike wrote them.
pub fn scores_csv(scores: &[Score]) -> String {
    let mut csv = String::from("image,laplacian_var,kept\n");
    for s in scores {
        writeln!(csv, "{},{:.2},{}", s.image, s.laplacian_var, s.kept)
            .expect("writing to a String");
    }
    csv
}

/// `extracted`, `kept` and `kept_ratio`.
pub fn metrics(scores: &[Score]) -> Metrics {
    let extracted = scores.len() as f64;
    let kept = scores.iter().filter(|s| s.kept).count() as f64;
    [
        ("extracted", extracted),
        ("kept", kept),
        ("kept_ratio", kept / extracted),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_owned(), v))
    .collect()
}

pub(crate) fn stage_error(message: impl Into<String>) -> Error {
    Error::Stage {
        stage: NAME,
        message: message.into(),
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;

    use super::*;
    use shoreline_core::tools::argv;
    use shoreline_core::tools::{Runtime, Tool, ToolSpec};

    fn frames(max_long_edge: Option<u32>) -> FramesConfig {
        FramesConfig {
            fps: 2.0,
            blur_threshold: 100.0,
            max_long_edge,
        }
    }

    fn fixture(name: &str) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures/frames")
            .join(name)
    }

    #[test]
    fn filter_caps_the_long_edge_for_either_orientation() {
        assert_eq!(
            video_filter(&frames(Some(3200))),
            "fps=2,scale='if(gte(iw,ih),min(3200,iw),-2)':'if(gte(iw,ih),-2,min(3200,ih))':flags=lanczos"
        );
        assert_eq!(video_filter(&frames(None)), "fps=2");
        let half = FramesConfig {
            fps: 0.5,
            ..frames(None)
        };
        assert_eq!(video_filter(&half), "fps=0.5");
    }

    #[test]
    fn extraction_writes_numbered_jpegs_and_mounts_input_and_output() {
        let inv = extract_invocation(
            Path::new("/data/raw/clip.mp4"),
            Path::new("/work/ds/frames/abc/images"),
            &frames(None),
        );
        let ffmpeg = Tool {
            name: "ffmpeg".into(),
            spec: ToolSpec {
                runtime: Runtime::Native,
                program: "ffmpeg".into(),
                image: None,
                gpu: false,
            },
        };
        assert_eq!(
            argv(&ffmpeg.command(&inv).unwrap()),
            [
                "ffmpeg",
                "-v",
                "error",
                "-nostdin",
                "-i",
                "/data/raw/clip.mp4",
                "-vf",
                "fps=2",
                "-q:v",
                "2",
                "-start_number",
                "1",
                "/work/ds/frames/abc/images/%05d.jpg",
            ]
        );
        assert_eq!(
            inv.mounts,
            [
                Path::new("/data/raw"),
                Path::new("/work/ds/frames/abc/images")
            ]
        );
    }

    #[test]
    fn select_moves_frames_below_the_threshold_aside() {
        let dir = tempfile::tempdir().unwrap();
        let (images, rejected) = (dir.path().join("images"), dir.path().join("rejected"));
        fs::create_dir(&images).unwrap();
        for (i, name) in ["sharp", "near_drop", "near_keep", "blurry"]
            .iter()
            .enumerate()
        {
            fs::copy(
                fixture(&format!("{name}.jpg")),
                images.join(format!("{:05}.jpg", i + 1)),
            )
            .unwrap();
        }
        fs::write(images.join("notes.txt"), "ignored").unwrap();

        let scores = select(&images, &rejected, 100.0).unwrap();
        let kept: Vec<_> = scores.iter().map(|s| (s.image.as_str(), s.kept)).collect();
        assert_eq!(
            kept,
            [
                ("00001.jpg", true),
                ("00002.jpg", false),
                ("00003.jpg", true),
                ("00004.jpg", false)
            ]
        );
        assert!(images.join("00001.jpg").is_file() && images.join("00003.jpg").is_file());
        assert!(rejected.join("00002.jpg").is_file() && rejected.join("00004.jpg").is_file());
        assert!(!images.join("00002.jpg").exists());
        assert!(scores[0].laplacian_var > 1000.0 && scores[3].laplacian_var < 10.0);
    }

    #[test]
    fn select_with_no_frames_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let err = select(dir.path(), &dir.path().join("rejected"), 100.0).unwrap_err();
        assert!(err.to_string().contains("no frames"), "{err}");
    }

    #[test]
    fn csv_and_metrics() {
        let scores = [
            Score {
                image: "00001.jpg".into(),
                laplacian_var: 225.6249,
                kept: true,
            },
            Score {
                image: "00002.jpg".into(),
                laplacian_var: 51.8051,
                kept: false,
            },
        ];
        assert_eq!(
            scores_csv(&scores),
            "image,laplacian_var,kept\n00001.jpg,225.62,true\n00002.jpg,51.81,false\n"
        );
        let m = metrics(&scores);
        assert_eq!(
            (m["extracted"], m["kept"], m["kept_ratio"]),
            (2.0, 1.0, 0.5)
        );
    }

    #[test]
    fn directory_input_is_not_supported_yet() {
        use shoreline_core::Tools;
        let dir = tempfile::tempdir().unwrap();
        let mut config = Config::load(
            &Path::new(env!("CARGO_MANIFEST_DIR")).join("../../configs/reposalmi-laajasalo.toml"),
        )
        .unwrap();
        config.dataset.input = dir.path().to_owned();
        let ctx = RunContext::new(
            config,
            Tools::from_table(toml::Table::new()).unwrap(),
            dir.path(),
        )
        .unwrap();
        let out = dir.path().join("out");
        fs::create_dir(&out).unwrap();
        let err = Frames.run(&ctx, &Inputs::new(), &out).unwrap_err();
        assert!(err.to_string().contains("#23"), "{err}");
    }

    /// Need ffmpeg on PATH: `cargo nextest run --profile integration`.
    mod integration {
        use std::process::Command;

        use super::*;
        use shoreline_core::Tools;
        use shoreline_core::pipeline::{self, StageResult};

        /// A 3 s, 10 fps, 640×360 test pattern, optionally blurred.
        fn video(dir: &Path, blur: bool) -> PathBuf {
            let path = dir.join(if blur { "blurred.mp4" } else { "sharp.mp4" });
            let filter = if blur { "gblur=sigma=8" } else { "null" };
            let status = Command::new("ffmpeg")
                .args(["-v", "error", "-nostdin", "-f", "lavfi"])
                .args(["-i", "testsrc2=size=640x360:rate=10:duration=3"])
                .args(["-vf", filter, "-c:v", "mpeg4", "-q:v", "2"])
                .arg(&path)
                .status()
                .expect("ffmpeg on PATH");
            assert!(status.success());
            path
        }

        fn context(dir: &Path, input: PathBuf) -> RunContext {
            let configs = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../configs");
            let dataset = configs.join("reposalmi-laajasalo.toml");
            let mut config = Config::load(&dataset).unwrap();
            config.dataset.input = input;
            config.frames.max_long_edge = Some(320);
            let tools = Tools::load(&dataset, None).unwrap();
            RunContext::new(config, tools, &dir.join("work")).unwrap()
        }

        fn run(ctx: &RunContext) -> Result<Vec<StageResult>> {
            pipeline::run(ctx, &[&Frames], &[], &mut |_| {})
        }

        #[test]
        fn extracts_scales_scores_and_caches() {
            let dir = tempfile::tempdir().unwrap();
            let ctx = context(dir.path(), video(dir.path(), false));
            let first = run(&ctx).unwrap().remove(0);
            assert!(!first.cached);
            assert_eq!(first.metrics["extracted"], 6.0, "{:?}", first.metrics);
            assert_eq!(first.metrics["kept"], 6.0, "a test pattern is sharp");
            let l = blur::decode_luma(&first.dir.join("images/00001.jpg")).unwrap();
            assert_eq!((l.width, l.height), (320, 180));
            let csv = fs::read_to_string(first.dir.join("frames.csv")).unwrap();
            assert_eq!(csv.lines().count(), 7);
            assert!(first.dir.join("logs/ffmpeg.log").is_file());

            let second = run(&ctx).unwrap().remove(0);
            assert!(second.cached);
            assert_eq!(second.fingerprint, first.fingerprint);
        }

        #[test]
        fn all_blurry_is_an_error() {
            let dir = tempfile::tempdir().unwrap();
            let ctx = context(dir.path(), video(dir.path(), true));
            let err = run(&ctx).unwrap_err();
            assert!(err.to_string().contains("blur_threshold"), "{err}");
        }
    }
}
