//! The stage contract. Concrete stages live in their own crates; the
//! [`pipeline`](crate::pipeline) runs them.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::error::io_error;
use crate::tools::ToolRecord;
use crate::{Config, Fingerprint, Result, Tools};

/// Outputs of the stages a stage depends on, keyed by stage name. Their
/// directories are absolute.
pub type Inputs = BTreeMap<&'static str, StageOutput>;

/// Scalar metrics reported by a stage and checked against gates.
pub type Metrics = BTreeMap<String, f64>;

#[derive(Debug, Clone, PartialEq)]
pub struct StageOutput {
    pub fingerprint: Fingerprint,
    pub dir: PathBuf,
}

/// What a stage run produced besides its files; recorded in the manifest.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct StageRun {
    pub tools: Vec<ToolRecord>,
    pub metrics: Metrics,
}

/// Per-run context shared by all stages.
#[derive(Debug)]
pub struct RunContext {
    pub config: Config,
    pub tools: Tools,
    /// `work/<dataset>/`
    pub work_dir: PathBuf,
}

impl RunContext {
    /// Context whose cache lives under `work_root/<dataset name>/`. The root is
    /// created and made canonical (absolute, no `..`, no symlinks): containers
    /// bind-mount stage directories at the same path, and Docker cleans a
    /// mount's target but not the arguments that name it, so a `..` in the
    /// path would point at an unmounted directory inside the container.
    pub fn new(config: Config, tools: Tools, work_root: &Path) -> Result<Self> {
        std::fs::create_dir_all(work_root).map_err(io_error(work_root))?;
        let work_root = std::fs::canonicalize(work_root).map_err(io_error(work_root))?;
        let work_dir = work_root.join(&config.dataset.name);
        Ok(Self {
            config,
            tools,
            work_dir,
        })
    }

    /// Cache directory: `work/<dataset>/<stage>/<fingerprint>/`.
    pub fn stage_dir(&self, stage: &str, fp: &Fingerprint) -> PathBuf {
        self.work_dir.join(stage).join(fp.short())
    }
}

pub trait Stage {
    fn name(&self) -> &'static str;
    fn deps(&self) -> &[&'static str];
    /// Config that determines the output, usually the stage's own section.
    /// The pipeline fingerprints the content of `dataset.input` itself.
    fn config(&self, config: &Config) -> serde_json::Value;
    /// Everything else that determines the output: the implementation
    /// revision, the versions of the tools the stage runs, and content hashes
    /// of other files it reads by path (for example model weights).
    fn version(&self, ctx: &RunContext) -> Result<String>;
    /// Write outputs into `out_dir`, which is absolute, exists and is empty.
    /// Tool logs must be written below it.
    fn run(&self, ctx: &RunContext, inputs: &Inputs, out_dir: &Path) -> Result<StageRun>;
    /// Quality gates on the recorded metrics: `Err` fails the run, `Ok`
    /// carries warnings. Evaluated on every run, cached or not, so changed
    /// thresholds apply to cached outputs.
    fn gate(&self, config: &Config, metrics: &Metrics) -> Result<Vec<String>> {
        let _ = (config, metrics);
        Ok(Vec::new())
    }
    /// The tool steps `run` executes for `config`, in order, named by the
    /// stems of their logs (`logs/<step>.log`). Progress reporting tells the
    /// current step by the newest log.
    fn steps(&self, config: &Config) -> Vec<String> {
        let _ = config;
        Vec::new()
    }
    /// Completed fraction (0 to 1) of `step`, from its log so far. `frames`
    /// is the number of frames the frames stage kept, when known. `None`
    /// when the log does not tell.
    fn progress(&self, step: &str, log: &str, frames: Option<usize>) -> Option<f64> {
        let _ = (step, log, frames);
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn work_dir_is_canonical_for_a_relative_root_with_parent_components() {
        let config = Config::load(
            &Path::new(env!("CARGO_MANIFEST_DIR")).join("../../configs/reposalmi-laajasalo.toml"),
        )
        .unwrap();
        let tools = Tools::from_table(toml::Table::new()).unwrap();
        let tmp = tempfile::tempdir().unwrap();
        // A relative path from the working directory into the temp dir, through
        // `..` and a not-yet-existing `work/`, as `--work ../x/work` would give.
        let cwd = std::env::current_dir().unwrap();
        let up: PathBuf = cwd.components().skip(1).map(|_| "..").collect();
        let root = up
            .join(tmp.path().strip_prefix("/").unwrap())
            .join("sub/../work");
        assert!(root.is_relative());

        let ctx = RunContext::new(config, tools, &root).unwrap();
        let expected = std::fs::canonicalize(tmp.path())
            .unwrap()
            .join("work/reposalmi-laajasalo");
        assert_eq!(ctx.work_dir, expected);
        assert!(
            ctx.work_dir
                .components()
                .all(|c| c != std::path::Component::ParentDir)
        );
    }
}
