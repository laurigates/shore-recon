//! Pipeline execution: run stages in dependency order, reusing cached
//! outputs (ADR-0004).
//!
//! A stage's fingerprint is computed here, not by the stage, from its name,
//! its config, its upstream fingerprints and its version, so no stage can
//! leave its lineage out.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::cache::{self, Manifest, Upstream};
use crate::dag::topo_order;
use crate::error::{Error, Result};
use crate::fingerprint::Fingerprint;
use crate::stage::{Inputs, Metrics, RunContext, Stage, StageOutput};

/// Outcome for one stage.
#[derive(Debug, Clone, PartialEq)]
pub struct StageResult {
    pub stage: &'static str,
    pub fingerprint: Fingerprint,
    pub dir: PathBuf,
    pub cached: bool,
    pub metrics: Metrics,
}

/// Progress, reported as it happens.
#[derive(Debug)]
pub enum Event<'a> {
    /// An interrupted run's directory (no manifest) was removed.
    Discarded {
        stage: &'static str,
        dir: &'a Path,
    },
    Running {
        stage: &'static str,
        dir: &'a Path,
    },
    Finished(&'a StageResult),
}

/// Run `targets` and everything they depend on (all `stages` when `targets`
/// is empty). Stops at the first stage error or failed gate; outputs already
/// written stay cached.
pub fn run(
    ctx: &RunContext,
    stages: &[&dyn Stage],
    targets: &[&str],
    on_event: &mut dyn FnMut(Event<'_>),
) -> Result<Vec<StageResult>> {
    let graph: Vec<(&str, &[&str])> = stages.iter().map(|s| (s.name(), s.deps())).collect();
    let order = topo_order(&graph)?;
    let selected = with_dependencies(&graph, targets)?;
    let by_name: BTreeMap<&str, &dyn Stage> = stages.iter().map(|s| (s.name(), *s)).collect();

    let mut outputs: BTreeMap<&str, StageOutput> = BTreeMap::new();
    let mut results = Vec::new();
    for name in order.into_iter().filter(|n| selected.contains(n)) {
        let stage = by_name[name];
        let inputs: Inputs = stage
            .deps()
            .iter()
            .map(|&dep| (dep, outputs[dep].clone()))
            .collect();
        let result = run_stage(ctx, stage, &inputs, on_event)?;
        on_event(Event::Finished(&result));
        stage.gate(&ctx.config, &result.metrics)?;
        outputs.insert(
            name,
            StageOutput {
                fingerprint: result.fingerprint,
                dir: result.dir.clone(),
            },
        );
        results.push(result);
    }
    Ok(results)
}

/// `targets` plus their transitive dependencies; every stage when empty.
fn with_dependencies<'a>(
    graph: &[(&'a str, &[&'a str])],
    targets: &[&str],
) -> Result<BTreeSet<&'a str>> {
    if targets.is_empty() {
        return Ok(graph.iter().map(|(name, _)| *name).collect());
    }
    let deps: BTreeMap<&str, &[&str]> = graph.iter().copied().collect();
    let mut selected = BTreeSet::new();
    let mut frontier: Vec<&str> = targets.to_vec();
    while let Some(name) = frontier.pop() {
        let (&name, stage_deps) = deps
            .get_key_value(name)
            .ok_or_else(|| Error::Dag(format!("unknown stage {name}")))?;
        if selected.insert(name) {
            frontier.extend(stage_deps.iter());
        }
    }
    Ok(selected)
}

/// Reuse the cached output for `stage` or run it into a fresh directory.
fn run_stage(
    ctx: &RunContext,
    stage: &dyn Stage,
    inputs: &Inputs,
    on_event: &mut dyn FnMut(Event<'_>),
) -> Result<StageResult> {
    let name = stage.name();
    let config = stage.config(&ctx.config);
    let version = stage.version(ctx)?;
    let upstream: Vec<Fingerprint> = inputs.values().map(|o| o.fingerprint).collect();
    let fingerprint = Fingerprint::compute(name, &config, &upstream, &version);
    let dir = ctx.stage_dir(name, &fingerprint);
    let result = |cached, metrics| StageResult {
        stage: name,
        fingerprint,
        dir: dir.clone(),
        cached,
        metrics,
    };

    if let Some(manifest) = cache::lookup(&dir, &fingerprint)? {
        return Ok(result(true, manifest.metrics));
    }
    let io = |path: &Path| {
        let path = path.to_owned();
        move |source| Error::Io { path, source }
    };
    if dir.exists() {
        std::fs::remove_dir_all(&dir).map_err(io(&dir))?;
        on_event(Event::Discarded {
            stage: name,
            dir: &dir,
        });
    }
    std::fs::create_dir_all(&dir).map_err(io(&dir))?;
    on_event(Event::Running {
        stage: name,
        dir: &dir,
    });

    let run = stage.run(ctx, inputs, &dir)?;
    let manifest = Manifest {
        stage: name.to_owned(),
        fingerprint,
        config,
        version,
        upstream: inputs
            .iter()
            .map(|(stage, o)| Upstream {
                stage: (*stage).to_owned(),
                fingerprint: o.fingerprint,
            })
            .collect(),
        tools: run.tools,
        metrics: run.metrics,
        created_unix: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_secs()),
    };
    cache::write_manifest(&dir, &manifest)?;
    Ok(result(false, manifest.metrics))
}

#[cfg(test)]
mod tests {
    use std::cell::{Cell, RefCell};
    use std::fs;
    use std::path::Path;

    use super::*;
    use crate::cache::{MANIFEST_FILE, lookup};
    use crate::stage::{Inputs, StageRun};
    use crate::tools::{Invocation, Runtime, Tool, ToolSpec};
    use crate::{Config, Error, Tools};

    /// Writes `out.txt` and reports `score`; gated on
    /// `gates.min_registered_ratio`.
    struct Fake {
        name: &'static str,
        deps: &'static [&'static str],
        config: RefCell<serde_json::Value>,
        version: RefCell<String>,
        score: Cell<f64>,
        fail: Cell<bool>,
        runs: Cell<u32>,
    }

    fn fake(name: &'static str, deps: &'static [&'static str]) -> Fake {
        Fake {
            name,
            deps,
            config: RefCell::new(serde_json::json!({"n": 1})),
            version: RefCell::new("1".into()),
            score: Cell::new(1.0),
            fail: Cell::new(false),
            runs: Cell::new(0),
        }
    }

    impl Stage for Fake {
        fn name(&self) -> &'static str {
            self.name
        }
        fn deps(&self) -> &[&'static str] {
            self.deps
        }
        fn config(&self, _: &Config) -> serde_json::Value {
            self.config.borrow().clone()
        }
        fn version(&self, _: &RunContext) -> Result<String> {
            Ok(self.version.borrow().clone())
        }
        fn run(&self, _: &RunContext, inputs: &Inputs, out_dir: &Path) -> Result<StageRun> {
            self.runs.set(self.runs.get() + 1);
            assert!(
                fs::read_dir(out_dir).unwrap().next().is_none(),
                "out_dir not empty"
            );
            assert_eq!(inputs.keys().copied().collect::<Vec<_>>(), self.deps);
            for input in inputs.values() {
                assert!(input.dir.join("out.txt").is_file(), "{input:?}");
                assert!(input.dir.ends_with(input.fingerprint.short()));
            }
            fs::write(out_dir.join("out.txt"), self.name).unwrap();
            if self.fail.get() {
                return Err(Error::Config(format!("{} failed", self.name)));
            }
            let log = out_dir.join("logs/echo.log");
            let echo = Tool {
                name: "sh".into(),
                spec: ToolSpec {
                    runtime: Runtime::Native,
                    program: "sh".into(),
                    image: None,
                    gpu: false,
                },
            };
            let record = echo.run(&Invocation::new(["-c", "echo ok"]), &log)?;
            Ok(StageRun {
                tools: vec![record],
                metrics: [("score".to_owned(), self.score.get())].into(),
            })
        }
        fn gate(&self, config: &Config, metrics: &Metrics) -> Result<()> {
            let min = config.gates.min_registered_ratio;
            if metrics["score"] < min {
                return Err(Error::Gate(format!("{}: score below {min}", self.name)));
            }
            Ok(())
        }
    }

    struct Env {
        _root: tempfile::TempDir,
        ctx: RunContext,
    }

    fn env() -> Env {
        let root = tempfile::tempdir().unwrap();
        let config = Config::load(
            &Path::new(env!("CARGO_MANIFEST_DIR")).join("../../configs/reposalmi-laajasalo.toml"),
        )
        .unwrap();
        let tools = Tools::from_table(toml::Table::new()).unwrap();
        let ctx = RunContext::new(config, tools, root.path());
        Env { _root: root, ctx }
    }

    /// a ← b ← c, and a ← d.
    fn stages() -> [Fake; 4] {
        [
            fake("a", &[]),
            fake("b", &["a"]),
            fake("c", &["b"]),
            fake("d", &["a"]),
        ]
    }

    fn refs(s: &[Fake]) -> Vec<&dyn Stage> {
        s.iter().map(|f| f as &dyn Stage).collect()
    }

    fn runs(s: &[Fake]) -> Vec<u32> {
        s.iter().map(|f| f.runs.get()).collect()
    }

    fn go(
        ctx: &RunContext,
        s: &[Fake],
        targets: &[&str],
    ) -> (Result<Vec<StageResult>>, Vec<String>) {
        let mut events = Vec::new();
        let result = run(ctx, &refs(s), targets, &mut |e| {
            events.push(match e {
                Event::Discarded { stage, .. } => format!("discard {stage}"),
                Event::Running { stage, .. } => format!("run {stage}"),
                Event::Finished(r) if r.cached => format!("cached {}", r.stage),
                Event::Finished(r) => format!("done {}", r.stage),
            })
        });
        (result, events)
    }

    fn by_name<'a>(results: &'a [StageResult], name: &str) -> &'a StageResult {
        results.iter().find(|r| r.stage == name).unwrap()
    }

    #[test]
    fn first_run_executes_in_order_and_second_is_a_full_cache_hit() {
        let e = env();
        let s = stages();
        let (first, events) = go(&e.ctx, &s, &[]);
        let first = first.unwrap();
        assert_eq!(
            events,
            [
                "run a", "done a", "run b", "done b", "run c", "done c", "run d", "done d"
            ]
        );
        assert!(first.iter().all(|r| !r.cached));

        let (second, events) = go(&e.ctx, &s, &[]);
        let second = second.unwrap();
        assert_eq!(events, ["cached a", "cached b", "cached c", "cached d"]);
        assert_eq!(runs(&s), [1, 1, 1, 1]);
        assert_eq!(second.len(), first.len(), "cached stages are reported too");
        assert!(second.iter().all(|r| r.cached));
        for (f, c) in first.iter().zip(&second) {
            assert_eq!(
                (f.fingerprint, &f.dir, &f.metrics),
                (c.fingerprint, &c.dir, &c.metrics)
            );
        }
    }

    #[test]
    fn output_dir_is_named_by_fingerprint_under_the_dataset() {
        let e = env();
        let s = stages();
        let results = go(&e.ctx, &s, &["a"]).0.unwrap();
        let a = by_name(&results, "a");
        assert_eq!(a.dir, e.ctx.work_dir.join("a").join(a.fingerprint.short()));
        assert!(e.ctx.work_dir.ends_with("reposalmi-laajasalo"));
    }

    #[test]
    fn config_change_reruns_the_stage_and_its_dependents_only() {
        let e = env();
        let s = stages();
        let before = go(&e.ctx, &s, &[]).0.unwrap();
        *s[1].config.borrow_mut() = serde_json::json!({"n": 2});
        let (after, events) = go(&e.ctx, &s, &[]);
        let after = after.unwrap();
        assert_eq!(
            events,
            ["cached a", "run b", "done b", "run c", "done c", "cached d"]
        );
        assert_eq!(runs(&s), [1, 2, 2, 1]);
        let cached: Vec<_> = after.iter().map(|r| (r.stage, r.cached)).collect();
        assert_eq!(
            cached,
            [("a", true), ("b", false), ("c", false), ("d", true)]
        );
        assert!(
            by_name(&before, "b").dir.join(MANIFEST_FILE).is_file(),
            "old output kept"
        );
        assert_ne!(
            by_name(&before, "c").fingerprint,
            by_name(&after, "c").fingerprint
        );
    }

    #[test]
    fn version_change_reruns_the_stage_and_its_dependents() {
        let e = env();
        let s = stages();
        go(&e.ctx, &s, &[]).0.unwrap();
        *s[0].version.borrow_mut() = "2".into();
        go(&e.ctx, &s, &[]).0.unwrap();
        assert_eq!(runs(&s), [2, 2, 2, 2]);
    }

    #[test]
    fn targets_run_only_their_dependencies() {
        let e = env();
        let s = stages();
        let results = go(&e.ctx, &s, &["b"]).0.unwrap();
        assert_eq!(
            results.iter().map(|r| r.stage).collect::<Vec<_>>(),
            ["a", "b"]
        );
        assert_eq!(runs(&s), [1, 1, 0, 0]);
    }

    #[test]
    fn targets_run_their_transitive_dependencies() {
        let e = env();
        let s = stages();
        let results = go(&e.ctx, &s, &["c"]).0.unwrap();
        assert_eq!(
            results.iter().map(|r| r.stage).collect::<Vec<_>>(),
            ["a", "b", "c"]
        );
        assert_eq!(runs(&s), [1, 1, 1, 0]);
    }

    #[test]
    fn stages_run_in_dependency_order_whatever_the_registration_order() {
        let e = env();
        let s = stages();
        let reversed: Vec<&dyn Stage> = s.iter().rev().map(|f| f as &dyn Stage).collect();
        let mut events = Vec::new();
        run(&e.ctx, &reversed, &[], &mut |ev| {
            if let Event::Running { stage, .. } = ev {
                events.push(stage);
            }
        })
        .unwrap();
        let pos = |name| events.iter().position(|&s| s == name).unwrap();
        assert_eq!(events.len(), 4);
        assert!(
            pos("a") < pos("b") && pos("b") < pos("c") && pos("a") < pos("d"),
            "{events:?}"
        );
    }

    #[test]
    fn unknown_target_or_dependency_is_an_error() {
        let e = env();
        let s = stages();
        assert!(matches!(go(&e.ctx, &s, &["x"]).0, Err(Error::Dag(m)) if m.contains("x")));
        let orphan = [fake("b", &["a"])];
        assert!(matches!(go(&e.ctx, &orphan, &[]).0, Err(Error::Dag(m)) if m.contains("unknown")));
        assert_eq!(runs(&orphan), [0]);
    }

    #[test]
    fn manifest_records_lineage_tools_and_metrics() {
        let e = env();
        let s = stages();
        let results = go(&e.ctx, &s, &[]).0.unwrap();
        let (a, b) = (by_name(&results, "a"), by_name(&results, "b"));
        let m = lookup(&b.dir, &b.fingerprint).unwrap().unwrap();
        assert_eq!(m.stage, "b");
        assert_eq!(m.config, serde_json::json!({"n": 1}));
        assert_eq!(m.version, "1");
        assert_eq!(m.upstream.len(), 1);
        assert_eq!(
            (m.upstream[0].stage.as_str(), m.upstream[0].fingerprint),
            ("a", a.fingerprint)
        );
        assert_eq!(m.tools.len(), 1);
        assert_eq!(m.tools[0].tool, "sh");
        assert_eq!(m.metrics, b.metrics);
        assert_eq!(
            b.fingerprint,
            Fingerprint::compute("b", &m.config, &[a.fingerprint], "1")
        );
    }

    #[test]
    fn interrupted_output_is_discarded_and_rerun() {
        let e = env();
        let s = stages();
        let a = go(&e.ctx, &s, &["a"]).0.unwrap().remove(0);
        fs::remove_file(a.dir.join(MANIFEST_FILE)).unwrap();
        fs::write(a.dir.join("stray.tmp"), "").unwrap();
        let (_, events) = go(&e.ctx, &s, &["a"]);
        assert_eq!(events, ["discard a", "run a", "done a"]);
        assert!(!a.dir.join("stray.tmp").exists());
        assert_eq!(runs(&s)[0], 2);
    }

    #[test]
    fn corrupt_manifest_is_an_error_and_left_in_place() {
        let e = env();
        let s = stages();
        let a = go(&e.ctx, &s, &["a"]).0.unwrap().remove(0);
        fs::write(a.dir.join(MANIFEST_FILE), "{").unwrap();
        assert!(matches!(go(&e.ctx, &s, &["a"]).0, Err(Error::Cache(_))));
        assert!(a.dir.join("out.txt").is_file());
        assert_eq!(runs(&s)[0], 1);
    }

    #[test]
    fn stage_failure_stops_the_run_without_a_manifest() {
        let e = env();
        let s = stages();
        s[1].fail.set(true);
        let (result, events) = go(&e.ctx, &s, &[]);
        assert!(matches!(result, Err(Error::Config(m)) if m == "b failed"));
        assert_eq!(events, ["run a", "done a", "run b"]);
        assert_eq!(runs(&s), [1, 1, 0, 0]);

        s[1].fail.set(false);
        let (_, events) = go(&e.ctx, &s, &[]);
        assert_eq!(events[..3], ["cached a", "discard b", "run b"]);
    }

    #[test]
    fn failed_gate_keeps_the_output_and_rechecks_cached_metrics() {
        let mut e = env();
        let s = stages();
        s[0].score.set(0.5);
        let (result, events) = go(&e.ctx, &s, &[]);
        assert!(matches!(result, Err(Error::Gate(m)) if m.contains("a: score")));
        assert_eq!(events, ["run a", "done a"]);

        // Still failing from the cache: gates apply to cached outputs too.
        assert!(matches!(go(&e.ctx, &s, &["a"]).0, Err(Error::Gate(_))));
        e.ctx.config.gates.min_registered_ratio = 0.4;
        go(&e.ctx, &s, &[]).0.unwrap();
        assert_eq!(runs(&s), [1, 1, 1, 1]);
    }
}
