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
/// Log stem of the extraction step.
const LOG: &str = "ffmpeg";

/// Bump when the output changes for the same config, input and ffmpeg.
const REVISION: u32 = 2;

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

    fn steps(&self, _: &Config) -> Vec<String> {
        vec![LOG.to_owned()]
    }

    fn run(
        &self,
        ctx: &RunContext,
        _inputs: &Inputs,
        out_dir: &Path,
        on_event: &mut dyn FnMut(shoreline_core::pipeline::Event<'_>),
    ) -> Result<StageRun> {
        let input = &ctx.config.dataset.input;
        if input.is_dir() {
            return Err(stage_error(format!(
                "{} is a directory; photo-directory input is not implemented yet (#23)",
                input.display()
            )));
        }
        // Through symlinks, so a container sees the real file under its mount.
        let input = std::fs::canonicalize(input).map_err(io_error(input))?;
        let images = out_dir.join("images");
        if images.to_string_lossy().contains('%') {
            return Err(stage_error(format!(
                "{} contains '%', which ffmpeg reads as part of its frame-number pattern; \
                 use a work directory without '%'",
                images.display()
            )));
        }
        std::fs::create_dir_all(&images).map_err(io_error(&images))?;

        let extract = extract_invocation(&input, &images, &ctx.config.frames);
        let record = ctx
            .tools
            .get("ffmpeg")?
            .run(&extract, &out_dir.join(format!("logs/{LOG}.log")))?;
        on_event(shoreline_core::pipeline::Event::Step {
            stage: NAME,
            message: "extracted frames",
        });

        let threshold = ctx.config.frames.blur_threshold;
        let scores = select(&images, &out_dir.join("rejected"), threshold)?;
        let kept = scores.iter().filter(|s| s.kept).count();
        on_event(shoreline_core::pipeline::Event::Step {
            stage: NAME,
            message: &format!("scored {} frames, kept {}", scores.len(), kept),
        });
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

/// Score every `*.jpg` in `images` in frame order and move those below
/// `threshold` into `rejected`.
pub fn select(images: &Path, rejected: &Path, threshold: f64) -> Result<Vec<Score>> {
    let mut names = Vec::new();
    for entry in std::fs::read_dir(images).map_err(io_error(images))? {
        let name = entry.map_err(io_error(images))?.file_name();
        if let Some(name) = name.to_str().filter(|n| n.ends_with(".jpg")) {
            names.push(name.to_owned());
        }
    }
    // Frame order: ffmpeg's %05d names grow a digit past 99999.
    names.sort_by(|a, b| (a.len(), a).cmp(&(b.len(), b)));
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

/// `image,laplacian_var,kept` rows (the M0 spike's columns). Scores are
/// written at full precision, so a rejected frame never shows a score at or
/// above the threshold.
pub fn scores_csv(scores: &[Score]) -> String {
    let mut csv = String::from("image,laplacian_var,kept\n");
    for s in scores {
        writeln!(csv, "{},{},{}", s.image, s.laplacian_var, s.kept).expect("writing to a String");
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
                remote: None,
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
            "image,laplacian_var,kept\n00001.jpg,225.6249,true\n00002.jpg,51.8051,false\n"
        );
        let m = metrics(&scores);
        assert_eq!(
            (m["extracted"], m["kept"], m["kept_ratio"]),
            (2.0, 1.0, 0.5)
        );
    }

    fn repo_config() -> Config {
        Config::load(
            &Path::new(env!("CARGO_MANIFEST_DIR")).join("../../configs/reposalmi-laajasalo.toml"),
        )
        .unwrap()
    }

    #[test]
    fn config_is_the_frames_section_and_tracks_every_field() {
        let config = repo_config();
        let base = Frames.config(&config);
        assert_eq!(base, serde_json::to_value(&config.frames).unwrap());
        let changes: [fn(&mut FramesConfig); 3] = [
            |f| f.fps = 3.0,
            |f| f.blur_threshold = 150.0,
            |f| f.max_long_edge = Some(1600),
        ];
        for change in changes {
            let mut changed = config.clone();
            change(&mut changed.frames);
            assert_ne!(Frames.config(&changed), base);
        }
    }

    /// An executable stand-in for ffmpeg. `-version` prints a sentinel line;
    /// otherwise it records its arguments in `args.txt` and copies the named
    /// fixtures to the output pattern's directory as 00001.jpg, 00002.jpg, …
    fn stub_ffmpeg(dir: &Path, fixtures: &[&str]) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let copies: String = fixtures
            .iter()
            .enumerate()
            .map(|(i, name)| {
                let src = fixture(&format!("{name}.jpg"));
                format!("cp '{}' \"$out/{:05}.jpg\"\n", src.display(), i + 1)
            })
            .collect();
        let script = format!(
            "#!/bin/sh\n\
             if [ \"$1\" = -version ]; then echo 'ffmpeg version STUB-1'; exit 0; fi\n\
             printf '%s\\n' \"$@\" > '{}'\n\
             for last; do :; done\n\
             out=$(dirname \"$last\")\n{copies}",
            dir.join("args.txt").display()
        );
        let path = dir.join("ffmpeg-stub");
        fs::write(&path, script).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        assert_ne!(fs::metadata(&path).unwrap().permissions().mode() & 0o111, 0);
        path
    }

    /// Context whose input is a symlink to a placeholder video and whose
    /// ffmpeg is the stub.
    fn stub_context(dir: &Path, fixtures: &[&str]) -> RunContext {
        use shoreline_core::Tools;
        let real = dir.join("real");
        fs::create_dir(&real).unwrap();
        fs::write(real.join("clip.mp4"), b"placeholder").unwrap();
        let link = dir.join("clip.mp4");
        std::os::unix::fs::symlink(real.join("clip.mp4"), &link).unwrap();
        let mut config = repo_config();
        config.dataset.input = link;
        let program = stub_ffmpeg(dir, fixtures);
        let table = format!(
            "[ffmpeg]\nruntime = \"native\"\nprogram = \"{}\"\n",
            program.display()
        );
        let tools = Tools::from_table(table.parse().unwrap()).unwrap();
        RunContext::new(config, tools, &dir.join("work")).unwrap()
    }

    fn out_dir(dir: &Path, name: &str) -> PathBuf {
        let out = dir.join(name);
        fs::create_dir(&out).unwrap();
        out
    }

    #[test]
    fn version_is_the_revision_and_the_ffmpeg_version_line() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = stub_context(dir.path(), &[]);
        assert_eq!(
            Frames.version(&ctx).unwrap(),
            "frames/2 ffmpeg version STUB-1"
        );
    }

    #[test]
    fn run_splits_frames_and_records_csv_log_and_the_resolved_input() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = stub_context(dir.path(), &["sharp", "near_drop", "near_keep"]);
        let out = out_dir(dir.path(), "out");
        let run = Frames.run(&ctx, &Inputs::new(), &out, &mut |_| {}).unwrap();

        assert_eq!((run.metrics["extracted"], run.metrics["kept"]), (3.0, 2.0));
        assert!(out.join("images/00001.jpg").is_file() && out.join("images/00003.jpg").is_file());
        assert!(out.join("rejected/00002.jpg").is_file());
        let csv = fs::read_to_string(out.join("frames.csv")).unwrap();
        assert_eq!(csv.lines().count(), 4, "{csv}");
        assert_eq!(run.tools[0].log, out.join("logs/ffmpeg.log"));
        assert!(run.tools[0].log.is_file());
        assert_eq!(Frames.steps(&ctx.config), ["ffmpeg"]);

        let args = fs::read_to_string(dir.path().join("args.txt")).unwrap();
        let args: Vec<_> = args.lines().collect();
        let input = args[args.iter().position(|a| *a == "-i").unwrap() + 1];
        let real = fs::canonicalize(dir.path().join("real/clip.mp4")).unwrap();
        assert_eq!(Path::new(input), real, "absolute and through the symlink");
    }

    #[test]
    fn run_that_keeps_nothing_fails_naming_the_threshold() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = stub_context(dir.path(), &["near_drop", "blurry"]);
        let out = out_dir(dir.path(), "out");
        let err = Frames
            .run(&ctx, &Inputs::new(), &out, &mut |_| {})
            .unwrap_err();
        assert!(err.to_string().contains("blur_threshold = 100"), "{err}");
        assert!(out.join("frames.csv").is_file(), "scores kept for review");
    }

    #[test]
    fn percent_in_the_output_path_is_refused_before_ffmpeg_runs() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = stub_context(dir.path(), &["sharp"]);
        let out = out_dir(dir.path(), "w%d");
        let err = Frames
            .run(&ctx, &Inputs::new(), &out, &mut |_| {})
            .unwrap_err();
        assert!(err.to_string().contains("'%'"), "{err}");
        assert!(!dir.path().join("args.txt").exists(), "ffmpeg did not run");
    }

    #[test]
    fn frames_stay_in_order_past_five_digits() {
        let dir = tempfile::tempdir().unwrap();
        let images = out_dir(dir.path(), "images");
        for name in ["99999", "100000", "10001"] {
            fs::copy(fixture("sharp.jpg"), images.join(format!("{name}.jpg"))).unwrap();
        }
        let scores = select(&images, &dir.path().join("rejected"), 100.0).unwrap();
        let order: Vec<_> = scores.iter().map(|s| s.image.as_str()).collect();
        assert_eq!(order, ["10001.jpg", "99999.jpg", "100000.jpg"]);
    }

    #[test]
    fn a_score_equal_to_the_threshold_is_kept() {
        let dir = tempfile::tempdir().unwrap();
        let images = out_dir(dir.path(), "images");
        fs::copy(fixture("near_keep.jpg"), images.join("00001.jpg")).unwrap();
        let score =
            blur::laplacian_variance(&blur::decode_luma(&images.join("00001.jpg")).unwrap());
        let scores = select(&images, &dir.path().join("rejected"), score).unwrap();
        assert!(scores[0].kept, "the spike keeps v >= threshold");
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
        let err = Frames
            .run(&ctx, &Inputs::new(), &out, &mut |_| {})
            .unwrap_err();
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
