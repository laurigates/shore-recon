//! The stage contract. Concrete stages live in their own crates.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::{Config, Fingerprint, Result};

/// Paths produced by upstream stages, keyed by stage name.
pub type Inputs = BTreeMap<&'static str, StageOutput>;

/// Scalar metrics reported by a stage and checked against gates.
pub type Metrics = BTreeMap<String, f64>;

#[derive(Debug, Clone)]
pub struct StageOutput {
    pub fingerprint: Fingerprint,
    pub dir: PathBuf,
}

/// Per-run context shared by all stages.
#[derive(Debug)]
pub struct RunContext {
    pub config: Config,
    /// `work/<dataset>/`
    pub work_dir: PathBuf,
}

impl RunContext {
    /// Cache directory: `work/<dataset>/<stage>/<fingerprint>/`.
    pub fn stage_dir(&self, stage: &str, fp: &Fingerprint) -> PathBuf {
        self.work_dir.join(stage).join(fp.short())
    }
}

pub trait Stage {
    fn name(&self) -> &'static str;
    fn deps(&self) -> &[&'static str];
    /// `hash(stage config, upstream fingerprints, tool version)`
    fn fingerprint(&self, ctx: &RunContext, inputs: &Inputs) -> Fingerprint;
    /// Write outputs into `out_dir`.
    fn run(&self, ctx: &RunContext, inputs: &Inputs, out_dir: &Path) -> Result<()>;
    /// Quality gates; `Err` fails the run.
    fn check(&self, ctx: &RunContext, out_dir: &Path) -> Result<Metrics>;
}
