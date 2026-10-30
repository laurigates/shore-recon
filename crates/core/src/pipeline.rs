//! Pipeline execution: run stages in dependency order, reusing cached
//! outputs (ADR-0004).
//!
//! A stage's fingerprint is computed here, not by the stage, from its name,
//! its config, its upstream fingerprints and its version, so no stage can
//! leave its lineage out. Stages without dependencies take the content
//! fingerprint of the dataset input as their upstream.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use crate::cache::{self, Manifest, Source, Upstream};
use crate::dag::topo_order;
use crate::error::{Error, Result, io_error};
use crate::fingerprint::Fingerprint;
use crate::history::Estimate;
use crate::stage::{Inputs, Metrics, RunContext, Stage, StageOutput};

/// Outcome for one stage.
#[derive(Debug, Clone, PartialEq)]
pub struct StageResult {
    pub stage: &'static str,
    pub fingerprint: Fingerprint,
    pub dir: PathBuf,
    pub cached: bool,
    pub metrics: Metrics,
    /// From the stage's gate; recomputed on every run, never cached.
    pub warnings: Vec<String>,
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
        /// Position in the run's stages, from 1.
        index: usize,
        /// Number of stages in the run, cached ones included.
        total: usize,
        /// From earlier runs of the stage under the work root.
        estimate: Option<Estimate>,
    },
    Finished(&'a StageResult),
    /// A sub-step reported by a running stage.
    Step {
        stage: &'static str,
        message: &'a str,
    },
    /// A gate passed with a warning (reported after `Finished`).
    Warning {
        stage: &'static str,
        message: &'a str,
    },
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
    let input = &ctx.config.dataset.input;
    let source = Source {
        path: input.clone(),
        fingerprint: Fingerprint::of_content(input)?,
    };

    let order: Vec<&str> = order.into_iter().filter(|n| selected.contains(n)).collect();
    // Every version before any stage runs: a missing or broken tool fails the
    // run at once, not after the stages before it.
    let versions = order
        .iter()
        .map(|&name| Ok((name, by_name[name].version(ctx)?)))
        .collect::<Result<BTreeMap<&str, String>>>()?;

    let mut outputs: BTreeMap<&str, StageOutput> = BTreeMap::new();
    let mut results = Vec::new();
    for (i, &name) in order.iter().enumerate() {
        let stage = by_name[name];
        let position = Position {
            index: i + 1,
            total: order.len(),
        };
        let inputs: Inputs = stage
            .deps()
            .iter()
            .map(|&dep| (dep, outputs[dep].clone()))
            .collect();
        let source = stage.deps().is_empty().then_some(&source);
        let mut result = run_stage(
            ctx,
            stage,
            position,
            &versions[name],
            &inputs,
            source,
            on_event,
        )?;
        on_event(Event::Finished(&result));
        result.warnings = stage.gate(&ctx.config, &result.metrics)?;
        for message in &result.warnings {
            on_event(Event::Warning {
                stage: name,
                message,
            });
        }
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

/// The stages `run` would run for `targets`, in order.
pub fn plan(stages: &[&dyn Stage], targets: &[&str]) -> Result<Vec<&'static str>> {
    let graph: Vec<(&str, &[&str])> = stages.iter().map(|s| (s.name(), s.deps())).collect();
    let selected = with_dependencies(&graph, targets)?;
    let order = topo_order(&graph)?;
    let names: BTreeMap<&str, &'static str> = stages.iter().map(|s| (s.name(), s.name())).collect();
    Ok(order
        .into_iter()
        .filter(|n| selected.contains(n))
        .map(|n| names[n])
        .collect())
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

/// Why an interrupted output could not be discarded. Permission errors are
/// usually root-owned files from a container that ran as root (#20).
fn discard_error(stage: &'static str, dir: &Path, source: std::io::Error) -> Error {
    if source.kind() != std::io::ErrorKind::PermissionDenied {
        return io_error(dir)(source);
    }
    Error::Stage {
        stage,
        message: format!(
            "cannot discard the interrupted output {dir}: {source}. Files written by a container \
             that ran as root (#20) need `sudo rm -rf {dir}`",
            dir = dir.display()
        ),
    }
}

#[derive(Debug, Clone, Copy)]
struct Position {
    index: usize,
    total: usize,
}

/// Reuse the cached output for `stage` or run it into a fresh directory.
fn run_stage(
    ctx: &RunContext,
    stage: &dyn Stage,
    position: Position,
    version: &str,
    inputs: &Inputs,
    source: Option<&Source>,
    on_event: &mut dyn FnMut(Event<'_>),
) -> Result<StageResult> {
    let name = stage.name();
    let config = stage.config(&ctx.config);
    let upstream: Vec<Fingerprint> = inputs
        .values()
        .map(|o| o.fingerprint)
        .chain(source.map(|s| s.fingerprint))
        .collect();
    let fingerprint = Fingerprint::compute(name, &config, &upstream, version);
    let dir = ctx.stage_dir(name, &fingerprint);
    let result = |cached, metrics| StageResult {
        stage: name,
        fingerprint,
        dir: dir.clone(),
        cached,
        metrics,
        warnings: Vec::new(),
    };

    if let Some(manifest) = cache::lookup(&dir, &fingerprint)? {
        return Ok(result(true, manifest.metrics));
    }
    if dir.exists() {
        std::fs::remove_dir_all(&dir).map_err(|e| discard_error(name, &dir, e))?;
        on_event(Event::Discarded {
            stage: name,
            dir: &dir,
        });
    }
    std::fs::create_dir_all(&dir).map_err(io_error(&dir))?;
    let work_root = ctx.work_dir.parent().unwrap_or(&ctx.work_dir);
    on_event(Event::Running {
        stage: name,
        dir: &dir,
        index: position.index,
        total: position.total,
        estimate: Estimate::of(work_root, name, Some(version)),
    });

    let started = Instant::now();
    let run = stage.run(ctx, inputs, &dir, on_event)?;
    let seconds = started.elapsed().as_secs_f64();
    // Logs relative to the output, so they resolve after the tree is copied.
    let tools = run
        .tools
        .into_iter()
        .map(|mut record| {
            let Ok(rel) = record.log.strip_prefix(&dir) else {
                return Err(Error::Cache(format!(
                    "{name}: tool log {} is outside {}",
                    record.log.display(),
                    dir.display()
                )));
            };
            record.log = rel.to_owned();
            Ok(record)
        })
        .collect::<Result<Vec<_>>>()?;
    let manifest = Manifest {
        stage: name.to_owned(),
        fingerprint,
        config,
        version: version.to_owned(),
        upstream: inputs
            .iter()
            .map(|(stage, o)| Upstream {
                stage: (*stage).to_owned(),
                fingerprint: o.fingerprint,
            })
            .collect(),
        source: source.cloned(),
        tools,
        metrics: run.metrics,
        created_unix: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_secs()),
        seconds: Some(seconds),
    };
    cache::publish(&dir, &manifest)?;
    Ok(result(false, manifest.metrics))
}

#[cfg(test)]
mod tests {
    use std::cell::{Cell, RefCell};
    use std::fs;
    use std::path::Path;

    use super::*;
    use crate::cache::{MANIFEST_FILE, Source, lookup};
    use crate::stage::{Inputs, StageRun};
    use crate::tools::{Invocation, Runtime, Tool, ToolSpec};
    use crate::{Config, Error, Tools};

    #[test]
    fn a_permission_error_on_discard_names_the_cause_and_the_fix() {
        let dir = Path::new("/work/d/masks/abc");
        let denied = std::io::Error::from(std::io::ErrorKind::PermissionDenied);
        let message = discard_error("masks", dir, denied).to_string();
        assert!(message.contains("#20"), "{message}");
        assert!(
            message.contains("sudo rm -rf /work/d/masks/abc"),
            "{message}"
        );
        let other = std::io::Error::from(std::io::ErrorKind::NotFound);
        assert!(matches!(
            discard_error("masks", dir, other),
            Error::Io { path, .. } if path == dir
        ));
    }

    /// Writes `out.txt` and reports `score` and `masked`; fails its gate when
    /// `score < gates.min_registered_ratio`, warns when
    /// `masked > gates.max_masked_fraction`.
    struct Fake {
        name: &'static str,
        deps: &'static [&'static str],
        config: RefCell<serde_json::Value>,
        version: RefCell<String>,
        score: Cell<f64>,
        masked: Cell<f64>,
        fail: Cell<bool>,
        log_outside: Cell<bool>,
        runs: Cell<u32>,
    }

    fn fake(name: &'static str, deps: &'static [&'static str]) -> Fake {
        Fake {
            name,
            deps,
            config: RefCell::new(serde_json::json!({"n": 1})),
            version: RefCell::new("1".into()),
            score: Cell::new(1.0),
            masked: Cell::new(0.0),
            fail: Cell::new(false),
            log_outside: Cell::new(false),
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
        fn version(&self, ctx: &RunContext) -> Result<String> {
            match self.version.borrow().as_str() {
                "missing" => ctx.tools.get(self.name).map(|_| unreachable!()),
                version => Ok(version.to_owned()),
            }
        }
        fn run(
            &self,
            _: &RunContext,
            inputs: &Inputs,
            out_dir: &Path,
            _on_event: &mut dyn FnMut(Event<'_>),
        ) -> Result<StageRun> {
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
            let log = match self.log_outside.get() {
                true => out_dir.parent().unwrap().join("echo.log"),
                false => out_dir.join("logs/echo.log"),
            };
            let echo = Tool {
                name: "sh".into(),
                spec: ToolSpec {
                    runtime: Runtime::Native,
                    program: "sh".into(),
                    image: None,
                    gpu: false,
                    remote: None,
                },
            };
            let record = echo.run(&Invocation::new(["-c", "echo ok"]), &log)?;
            Ok(StageRun {
                tools: vec![record],
                metrics: [
                    ("score".to_owned(), self.score.get()),
                    ("masked".to_owned(), self.masked.get()),
                ]
                .into(),
            })
        }
        fn gate(&self, config: &Config, metrics: &Metrics) -> Result<Vec<String>> {
            let min = config.gates.min_registered_ratio;
            if metrics["score"] < min {
                return Err(Error::Gate(format!("{}: score below {min}", self.name)));
            }
            let max = config.gates.max_masked_fraction;
            Ok(match metrics["masked"] > max {
                true => vec![format!("masked above {max}")],
                false => Vec::new(),
            })
        }
    }

    struct Env {
        _root: tempfile::TempDir,
        ctx: RunContext,
    }

    fn env() -> Env {
        let root = tempfile::tempdir().unwrap();
        let mut config = Config::load(
            &Path::new(env!("CARGO_MANIFEST_DIR")).join("../../configs/reposalmi-laajasalo.toml"),
        )
        .unwrap();
        config.dataset.input = root.path().join("input.mp4");
        fs::write(&config.dataset.input, b"video").unwrap();
        let tools = Tools::from_table(toml::Table::new()).unwrap();
        let ctx = RunContext::new(config, tools, &root.path().join("work")).unwrap();
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
                Event::Warning { stage, message } => format!("warn {stage}: {message}"),
                Event::Step { stage, message } => format!("step {stage}: {message}"),
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
    fn running_reports_the_position_and_an_estimate_from_earlier_runs() {
        let e = env();
        let s = stages();
        let running = |targets: &[&str]| {
            let mut seen = Vec::new();
            run(&e.ctx, &refs(&s), targets, &mut |ev| {
                if let Event::Running {
                    stage,
                    index,
                    total,
                    estimate,
                    ..
                } = ev
                {
                    seen.push((stage, index, total, estimate.map(|e| e.n)));
                }
            })
            .unwrap();
            seen
        };
        assert_eq!(
            running(&["c"]),
            [("a", 1, 3, None), ("b", 2, 3, None), ("c", 3, 3, None)]
        );
        // Positions count cached stages: b and c rerun as 2/3 and 3/3.
        *s[1].config.borrow_mut() = serde_json::json!({"n": 2});
        assert_eq!(
            running(&["c"]),
            [("b", 2, 3, Some(1)), ("c", 3, 3, Some(1))]
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
        assert_eq!(
            m.tools[0].log,
            Path::new("logs/echo.log"),
            "relative to the output"
        );
        assert_eq!(m.source, None, "only root stages record the source");
        assert_eq!(m.metrics, b.metrics);
        assert!(m.seconds.is_some_and(|s| s > 0.0), "{:?}", m.seconds);
        assert_eq!(
            b.fingerprint,
            Fingerprint::compute("b", &m.config, &[a.fingerprint], "1")
        );
    }

    #[test]
    fn root_stages_take_the_input_content_as_upstream() {
        let e = env();
        let s = stages();
        let a = go(&e.ctx, &s, &["a"]).0.unwrap().remove(0);
        let m = lookup(&a.dir, &a.fingerprint).unwrap().unwrap();
        let input = &e.ctx.config.dataset.input;
        let source = Fingerprint::of_content(input).unwrap();
        assert_eq!(
            m.source,
            Some(Source {
                path: input.clone(),
                fingerprint: source
            })
        );
        assert_eq!(
            a.fingerprint,
            Fingerprint::compute("a", &m.config, &[source], "1")
        );
    }

    #[test]
    fn input_content_change_reruns_everything_and_reverting_hits_the_cache() {
        let e = env();
        let s = stages();
        go(&e.ctx, &s, &[]).0.unwrap();
        fs::write(&e.ctx.config.dataset.input, b"another capture").unwrap();
        let (_, events) = go(&e.ctx, &s, &[]);
        assert!(
            events.iter().all(|e| !e.starts_with("cached")),
            "{events:?}"
        );
        assert_eq!(runs(&s), [2, 2, 2, 2]);
        fs::write(&e.ctx.config.dataset.input, b"video").unwrap();
        let (_, events) = go(&e.ctx, &s, &[]);
        assert!(events.iter().all(|e| e.starts_with("cached")), "{events:?}");
    }

    #[test]
    fn missing_input_fails_before_any_stage_runs() {
        let e = env();
        let s = stages();
        fs::remove_file(&e.ctx.config.dataset.input).unwrap();
        let (result, events) = go(&e.ctx, &s, &[]);
        let err = result.unwrap_err();
        assert!(
            matches!(&err, Error::Io { path, .. } if *path == e.ctx.config.dataset.input),
            "{err}"
        );
        assert!(events.is_empty());
        assert_eq!(runs(&s), [0, 0, 0, 0]);
    }

    #[test]
    fn warnings_follow_current_thresholds_on_fresh_and_cached_runs() {
        let mut e = env();
        let s = stages();
        s[3].masked.set(0.8);
        let (results, events) = go(&e.ctx, &s, &["d"]);
        assert_eq!(
            events,
            [
                "run a",
                "done a",
                "run d",
                "done d",
                "warn d: masked above 0.7"
            ]
        );
        assert_eq!(
            by_name(&results.unwrap(), "d").warnings,
            ["masked above 0.7"]
        );

        let (_, events) = go(&e.ctx, &s, &["d"]);
        assert_eq!(events, ["cached a", "cached d", "warn d: masked above 0.7"]);
        e.ctx.config.gates.max_masked_fraction = 0.9;
        let (results, events) = go(&e.ctx, &s, &["d"]);
        assert_eq!(events, ["cached a", "cached d"]);
        assert!(results.unwrap().iter().all(|r| r.warnings.is_empty()));
    }

    #[test]
    fn tool_log_outside_the_output_is_an_error_and_publishes_nothing() {
        let e = env();
        let s = stages();
        s[0].log_outside.set(true);
        let (result, _) = go(&e.ctx, &s, &["a"]);
        let err = result.unwrap_err();
        assert!(
            matches!(&err, Error::Cache(m) if m.contains("outside")),
            "{err}"
        );
        s[0].log_outside.set(false);
        let (_, events) = go(&e.ctx, &s, &["a"]);
        assert_eq!(events, ["discard a", "run a", "done a"]);
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
    fn a_missing_tool_fails_before_any_stage_runs() {
        let e = env();
        let s = stages();
        s[2].version.replace("missing".into());
        let (result, events) = go(&e.ctx, &s, &[]);
        assert!(matches!(result, Err(Error::Tools(m)) if m.contains("no [c] table")));
        assert!(events.is_empty());
        assert_eq!(runs(&s), [0, 0, 0, 0]);

        // Only the selected stages need their tools.
        go(&e.ctx, &s, &["d"]).0.unwrap();
        assert_eq!(runs(&s), [1, 0, 0, 1]);
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
    struct Stepper;

    impl Stage for Stepper {
        fn name(&self) -> &'static str {
            "step"
        }
        fn deps(&self) -> &[&'static str] {
            &[]
        }
        fn config(&self, _: &Config) -> serde_json::Value {
            serde_json::json!({"n": 1})
        }
        fn version(&self, _: &RunContext) -> Result<String> {
            Ok("1".into())
        }
        fn run(
            &self,
            _: &RunContext,
            _: &Inputs,
            out_dir: &Path,
            on_event: &mut dyn FnMut(Event<'_>),
        ) -> Result<StageRun> {
            std::fs::write(out_dir.join("out.txt"), "step").unwrap();
            on_event(Event::Step {
                stage: "step",
                message: "working",
            });
            Ok(StageRun::default())
        }
    }

    #[test]
    fn step_events_arrive_between_running_and_finished() {
        let e = env();
        // Stepper is a root stage; reuse env config. Need a stage list.
        let stepper = Stepper;
        let stages: [&dyn Stage; 1] = [&stepper];
        let mut events = Vec::new();
        run(&e.ctx, &stages, &[], &mut |ev| {
            events.push(match ev {
                Event::Running { stage, .. } => format!("run {stage}"),
                Event::Step { stage, message } => format!("step {stage}: {message}"),
                Event::Finished(r) => format!("done {}", r.stage),
                _ => String::new(),
            })
        })
        .unwrap();
        let run_pos = events.iter().position(|e| e == "run step").unwrap();
        let step_pos = events
            .iter()
            .position(|e| e == "step step: working")
            .unwrap();
        let done_pos = events.iter().position(|e| e == "done step").unwrap();
        assert!(run_pos < step_pos && step_pos < done_pos, "{events:?}");
    }
}
