//! The stage contract. Concrete stages live in their own crates; the
//! [`pipeline`](crate::pipeline) runs them.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::tools::ToolRecord;
use crate::{Config, Fingerprint, Result, Tools};

/// Outputs of the stages a stage depends on, keyed by stage name.
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
    /// Context whose cache lives under `work_root/<dataset name>/`.
    pub fn new(config: Config, tools: Tools, work_root: &Path) -> Self {
        let work_dir = work_root.join(&config.dataset.name);
        Self {
            config,
            tools,
            work_dir,
        }
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
    fn config(&self, config: &Config) -> serde_json::Value;
    /// Everything else that determines the output: the implementation
    /// revision and the versions of the tools the stage runs.
    fn version(&self, ctx: &RunContext) -> Result<String>;
    /// Write outputs into `out_dir`, which exists and is empty.
    fn run(&self, ctx: &RunContext, inputs: &Inputs, out_dir: &Path) -> Result<StageRun>;
    /// Quality gates on the recorded metrics; `Err` fails the run. Evaluated
    /// on every run, cached or not, so changed thresholds apply to cached
    /// outputs.
    fn gate(&self, config: &Config, metrics: &Metrics) -> Result<()> {
        let _ = (config, metrics);
        Ok(())
    }
}
