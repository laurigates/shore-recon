//! `export` stage: the v1 deliverables in one directory, with `report.json`
//! (kickoff § export / report, ADR-0008, #39).
//!
//! Links (or copies, across file systems) `model.glb`, `splat.ply` and
//! `water_plane.json` from post and the videos from video, and writes
//! `report.json`: the resolved config and its fingerprint, the source input,
//! and for every stage its fingerprint, version, metrics and tools, read from
//! the stage manifests. Tool records keep the tool, runtime, image and run
//! time; argument lists and hosts stay in the manifests.

use std::path::Path;

use serde::Serialize;
use shoreline_core::cache::{self, Source};
use shoreline_core::error::io_error;
use shoreline_core::stage::{Inputs, Metrics, StageOutput, StageRun};
use shoreline_core::tools::Runtime;
use shoreline_core::{Config, Error, Fingerprint, Result, RunContext, Stage};

pub const NAME: &str = "export";

/// Bump when the output changes for the same config and inputs.
const REVISION: u32 = 1;

/// Every stage that ran, in pipeline order: the report covers them all. The
/// order with videos is the one before the video stage became optional, so
/// those exports keep their fingerprints (ADR-0017).
const DEPS_WITH_VIDEOS: &[&str] = &["frames", "masks", "sfm", "dense", "splat", "post", "video"];
const DEPS_MODELS: &[&str] = &["frames", "masks", "sfm", "dense", "splat", "post"];

/// The post stage's deliverables.
pub const POST_FILES: &[&str] = &["model.glb", "splat.ply", "water_plane.json"];

/// `(stage, file)` deliverables, copied under the same name: post's files,
/// then with `videos` the processing-step clips and every video the config's
/// views produce.
pub fn deliverables(config: &Config, videos: bool) -> Vec<(&'static str, String)> {
    let post = POST_FILES.iter().map(|f| ("post", (*f).to_owned()));
    let video = shoreline_video::step_files()
        .into_iter()
        .chain(shoreline_video::video_files(&config.video))
        .map(|f| ("video", f))
        .filter(|_| videos);
    post.chain(video).collect()
}

pub const REPORT: &str = "report.json";

/// Metric names.
pub const DELIVERABLE_MIB: &str = "deliverable_mib";

/// The export stage, with or without the video stage's output
/// (`export.videos`; build it with [`Export::for_config`]).
pub struct Export {
    pub videos: bool,
}

impl Export {
    pub const MODELS: Export = Export { videos: false };
    pub const WITH_VIDEOS: Export = Export { videos: true };

    pub fn for_config(config: &Config) -> &'static Export {
        if config.export.videos {
            &Self::WITH_VIDEOS
        } else {
            &Self::MODELS
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ToolSummary {
    pub tool: String,
    pub runtime: Runtime,
    pub image: Option<String>,
    pub seconds: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct StageSummary {
    pub stage: String,
    pub fingerprint: Fingerprint,
    pub version: String,
    pub metrics: Metrics,
    pub tools: Vec<ToolSummary>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Deliverable {
    pub file: String,
    pub bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Report {
    pub dataset: String,
    pub source: Option<Source>,
    /// Fingerprint of the resolved config, independent of inputs and tools.
    pub config_fingerprint: Fingerprint,
    pub config: Config,
    pub stages: Vec<StageSummary>,
    pub water_plane: serde_json::Value,
    pub deliverables: Vec<Deliverable>,
}

fn stage_error(message: impl Into<String>) -> Error {
    Error::Stage {
        stage: NAME,
        message: message.into(),
    }
}

fn summary(name: &str, input: &StageOutput) -> Result<(StageSummary, Option<Source>)> {
    let m = cache::lookup(&input.dir, &input.fingerprint)?
        .ok_or_else(|| stage_error(format!("no manifest for {name} in {}", input.dir.display())))?;
    let tools = m
        .tools
        .into_iter()
        .map(|t| ToolSummary {
            tool: t.tool,
            runtime: t.runtime,
            image: t.image,
            seconds: t.seconds,
        })
        .collect();
    let summary = StageSummary {
        stage: m.stage,
        fingerprint: m.fingerprint,
        version: m.version,
        metrics: m.metrics,
        tools,
    };
    Ok((summary, m.source))
}

/// Hard-link `from` to `to`, or copy it when linking fails (another file
/// system).
fn link_or_copy(from: &Path, to: &Path) -> Result<()> {
    if std::fs::hard_link(from, to).is_ok() {
        return Ok(());
    }
    std::fs::copy(from, to).map_err(io_error(from))?;
    Ok(())
}

/// Copy an export's deliverables and `report.json` from `export_dir` into
/// `out` (created if needed), replacing files of the same name and leaving
/// any other file alone. Copies, not links, so editing them never touches
/// the cache. Each file is written to a temporary name, then renamed.
pub fn copy_out(
    export_dir: &Path,
    out: &Path,
    config: &Config,
    videos: bool,
) -> Result<Vec<std::path::PathBuf>> {
    std::fs::create_dir_all(out).map_err(io_error(out))?;
    let files = deliverables(config, videos)
        .into_iter()
        .map(|(_, f)| f)
        .chain([REPORT.to_owned()]);
    let mut copied = Vec::new();
    for file in files {
        let plain = Path::new(&file)
            .file_name()
            .is_some_and(|n| n == std::ffi::OsStr::new(&file));
        if !plain {
            return Err(stage_error(format!("refusing to copy out {file}")));
        }
        let from = export_dir.join(&file);
        let to = out.join(&file);
        let tmp = out.join(format!(".{file}.tmp"));
        std::fs::copy(&from, &tmp).map_err(io_error(&from))?;
        std::fs::rename(&tmp, &to).map_err(io_error(&to))?;
        copied.push(to);
    }
    Ok(copied)
}

impl Stage for Export {
    fn name(&self) -> &'static str {
        NAME
    }

    fn deps(&self) -> &[&'static str] {
        if self.videos {
            DEPS_WITH_VIDEOS
        } else {
            DEPS_MODELS
        }
    }

    fn config(&self, _config: &Config) -> serde_json::Value {
        serde_json::Value::Null
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
        let mut delivered = Vec::new();
        for (stage, file) in deliverables(&ctx.config, self.videos) {
            let from = inputs[stage].dir.join(&file);
            let to = out_dir.join(&file);
            link_or_copy(&from, &to)?;
            let bytes = std::fs::metadata(&to).map_err(io_error(&to))?.len();
            delivered.push(Deliverable { file, bytes });
        }

        let mut stages = Vec::new();
        let mut source = None;
        for &name in self.deps() {
            let (s, src) = summary(name, &inputs[name])?;
            source = source.or(src);
            stages.push(s);
        }
        let plane_path = out_dir.join("water_plane.json");
        let plane_text = std::fs::read_to_string(&plane_path).map_err(io_error(&plane_path))?;
        let water_plane = serde_json::from_str(&plane_text)
            .map_err(|e| stage_error(format!("{}: {e}", plane_path.display())))?;
        let report = Report {
            dataset: ctx.config.dataset.name.clone(),
            source,
            config_fingerprint: Fingerprint::compute("config", &ctx.config, &[], ""),
            config: ctx.config.clone(),
            stages,
            water_plane,
            deliverables: delivered,
        };
        let path = out_dir.join(REPORT);
        let json = serde_json::to_string_pretty(&report).expect("report serialises");
        std::fs::write(&path, json + "\n").map_err(io_error(&path))?;

        let total: u64 = report.deliverables.iter().map(|d| d.bytes).sum();
        let metrics = [(DELIVERABLE_MIB.to_owned(), total as f64 / (1024.0 * 1024.0))].into();
        Ok(StageRun {
            tools: Vec::new(),
            metrics,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use shoreline_core::cache::{Manifest, Upstream};
    use shoreline_core::tools::ToolRecord;
    use std::path::PathBuf;

    fn config() -> Config {
        Config::load(
            &Path::new(env!("CARGO_MANIFEST_DIR")).join("../../configs/reposalmi-laajasalo.toml"),
        )
        .unwrap()
    }

    /// A completed stage output under `root/<stage>` holding `files`.
    fn output(root: &Path, stage: &str, files: &[(&str, &str)]) -> StageOutput {
        let dir = root.join(stage);
        std::fs::create_dir_all(&dir).unwrap();
        for (name, content) in files {
            std::fs::write(dir.join(name), content).unwrap();
        }
        let fingerprint = Fingerprint::compute(stage, &serde_json::Value::Null, &[], "t");
        let manifest = Manifest {
            stage: stage.to_owned(),
            fingerprint,
            config: serde_json::Value::Null,
            version: format!("{stage}/1"),
            upstream: Vec::<Upstream>::new(),
            source: (stage == "frames").then(|| Source {
                path: PathBuf::from("data/raw/x.mp4"),
                fingerprint,
            }),
            tools: vec![ToolRecord {
                tool: "t".into(),
                runtime: Runtime::Native,
                image: None,
                host: Some("gpu-host".into()),
                argv: vec!["/secret/path".into()],
                seconds: 1.5,
                log: PathBuf::from("logs/t.log"),
            }],
            metrics: [(format!("{stage}_n"), 1.0)].into(),
            created_unix: 0,
            seconds: None,
        };
        cache::publish(&dir, &manifest).unwrap();
        StageOutput { fingerprint, dir }
    }

    #[test]
    fn run_gathers_deliverables_and_every_stage_into_the_report() {
        gather(&Export::WITH_VIDEOS);
        gather(&Export::MODELS);
    }

    fn gather(export: &Export) {
        let tmp = tempfile::tempdir().unwrap();
        let mut inputs = Inputs::new();
        let all = deliverables(&config(), export.videos);
        for &stage in export.deps() {
            let files: Vec<(&str, &str)> = all
                .iter()
                .filter(|(s, _)| *s == stage)
                .map(|(_, f)| {
                    let content = if f.ends_with(".json") {
                        "{\"plane\": 1}"
                    } else {
                        "data"
                    };
                    (f.as_str(), content)
                })
                .collect();
            inputs.insert(stage, output(tmp.path(), stage, &files));
        }
        let tools = shoreline_core::Tools::from_table(toml::Table::new()).unwrap();
        let ctx = RunContext::new(config(), tools, &tmp.path().join("work")).unwrap();
        let out = tmp.path().join("out");
        std::fs::create_dir_all(&out).unwrap();
        let run = export.run(&ctx, &inputs, &out, &mut |_| {}).unwrap();

        for (_, f) in &all {
            assert!(out.join(f).is_file(), "{f}");
        }
        assert_eq!(
            all.iter().any(|(_, f)| f == "mesh-drone.mp4"),
            export.videos
        );
        assert_eq!(
            all.iter().any(|(_, f)| f == "step-3-sparse.mp4"),
            export.videos
        );
        if !export.videos {
            assert_eq!(all.len(), POST_FILES.len());
        }
        let report: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(out.join(REPORT)).unwrap()).unwrap();
        assert_eq!(report["dataset"], "reposalmi-laajasalo");
        assert_eq!(report["source"]["path"], "data/raw/x.mp4");
        assert_eq!(report["water_plane"]["plane"], 1);
        let stages: Vec<_> = report["stages"]
            .as_array()
            .unwrap()
            .iter()
            .map(|s| s["stage"].clone())
            .collect();
        assert_eq!(
            stages,
            export
                .deps()
                .iter()
                .map(|s| serde_json::json!(s))
                .collect::<Vec<_>>()
        );
        assert_eq!(report["stages"][2]["metrics"]["sfm_n"], 1.0);
        let tool = &report["stages"][0]["tools"][0];
        assert_eq!(tool["seconds"], 1.5);
        assert!(tool.get("argv").is_none() && tool.get("host").is_none());
        assert_eq!(report["deliverables"].as_array().unwrap().len(), all.len());
        assert!(run.metrics[DELIVERABLE_MIB] > 0.0);
    }

    #[test]
    fn export_with_videos_keeps_the_previous_dependency_order() {
        // The fingerprint hashes upstreams in deps() order: existing exports
        // with videos stay cached only while this order holds.
        assert_eq!(
            Export::WITH_VIDEOS.deps(),
            ["frames", "masks", "sfm", "dense", "splat", "post", "video"]
        );
        assert_eq!(Export::MODELS.deps(), &Export::WITH_VIDEOS.deps()[..6]);
        let mut cfg = config();
        assert!(Export::for_config(&cfg).videos);
        cfg.export.videos = false;
        assert!(!Export::for_config(&cfg).videos);
    }

    #[test]
    fn copy_out_copies_deliverables_and_keeps_other_files() {
        let tmp = tempfile::tempdir().unwrap();
        let export = tmp.path().join("export");
        std::fs::create_dir_all(&export).unwrap();
        for f in POST_FILES.iter().chain([&REPORT]) {
            std::fs::write(export.join(f), "new").unwrap();
        }
        let out = tmp.path().join("out");
        std::fs::create_dir_all(&out).unwrap();
        std::fs::write(out.join("model.glb"), "old").unwrap();
        std::fs::write(out.join("notes.txt"), "mine").unwrap();

        let copied = copy_out(&export, &out, &config(), false).unwrap();
        assert_eq!(copied.len(), POST_FILES.len() + 1);
        assert_eq!(
            std::fs::read_to_string(out.join("model.glb")).unwrap(),
            "new"
        );
        assert_eq!(
            std::fs::read_to_string(out.join("notes.txt")).unwrap(),
            "mine"
        );
        assert!(
            std::fs::read_dir(&out)
                .unwrap()
                .all(|e| { !e.unwrap().file_name().to_string_lossy().ends_with(".tmp") })
        );
        // A copy, not a hard link: editing it leaves the cache alone.
        std::fs::write(out.join("splat.ply"), "edited").unwrap();
        assert_eq!(
            std::fs::read_to_string(export.join("splat.ply")).unwrap(),
            "new"
        );
    }

    #[test]
    fn copy_out_fails_on_a_missing_deliverable() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(copy_out(tmp.path(), &tmp.path().join("out"), &config(), true).is_err());
    }
}
