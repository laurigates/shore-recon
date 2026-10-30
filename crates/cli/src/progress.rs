//! Run progress on stderr (ADR-0018): two live lines on a terminal, plain
//! start, step, heartbeat and finish lines otherwise.
//!
//! The current step is the stage's newest `logs/<step>.log`, which every tool
//! writes locally (remote tools stream into it over ssh). A watcher thread
//! reads it every half second for the step, the stage's progress figure and
//! the last line printed.

use std::collections::{BTreeMap, BTreeSet};
use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant, SystemTime};

use indicatif::{MultiProgress, ProgressBar, ProgressDrawTarget, ProgressStyle};
use shoreline_core::history::{Estimate, elapsed, rough};
use shoreline_core::pipeline::Event;
use shoreline_core::{Config, Stage};

/// How often the watcher reads the log.
const POLL: Duration = Duration::from_millis(500);
/// How often plain mode reports a running stage.
const HEARTBEAT: Duration = Duration::from_secs(60);

pub type SyncStage = &'static (dyn Stage + Sync);

/// One planned stage.
struct Planned {
    stage: SyncStage,
    steps: Vec<String>,
    estimate: Option<Estimate>,
}

struct Current {
    started: Instant,
    /// The two live lines, when live.
    bars: Option<[ProgressBar; 2]>,
    estimate: Option<Estimate>,
    stop: mpsc::Sender<()>,
    watcher: Option<JoinHandle<()>>,
}

/// Turns pipeline events into progress output and the end-of-run summary.
pub struct Reporter {
    live: Option<MultiProgress>,
    plan: Vec<&'static str>,
    planned: BTreeMap<&'static str, Planned>,
    started: Instant,
    ran: BTreeSet<&'static str>,
    /// `(stage, seconds)`, `None` for a cached stage.
    rows: Vec<(&'static str, Option<f64>)>,
    current: Option<Current>,
    /// Frames the frames stage kept, for the progress of later stages.
    frames: Option<usize>,
}

impl Reporter {
    /// For the stages `plan` names, in order, with estimates from the cache
    /// under `work_root`. Live output when stderr is a terminal.
    pub fn new(
        stages: &[SyncStage],
        plan: &[&'static str],
        config: &Config,
        work_root: &Path,
        versions: Option<&std::collections::BTreeMap<&'static str, String>>,
    ) -> Self {
        let planned = stages
            .iter()
            .filter(|s| plan.contains(&s.name()))
            .map(|&stage| {
                let estimate = match versions.and_then(|v| v.get(stage.name())) {
                    Some(version) => Estimate::of(work_root, stage.name(), Some(version.as_str())),
                    None => Estimate::of(work_root, stage.name(), None),
                };
                let planned = Planned {
                    stage,
                    steps: stage.steps(config),
                    estimate,
                };
                (stage.name(), planned)
            })
            .collect();
        let live = std::io::stderr()
            .is_terminal()
            .then(|| MultiProgress::with_draw_target(ProgressDrawTarget::stderr()));
        Self {
            live,
            plan: plan.to_vec(),
            planned,
            started: Instant::now(),
            ran: BTreeSet::new(),
            rows: Vec::new(),
            current: None,
            frames: None,
        }
    }

    fn print(&self, line: &str) {
        match &self.live {
            Some(multi) => multi.suspend(|| eprintln!("{line}")),
            None => eprintln!("{line}"),
        }
    }

    pub fn event(&mut self, event: Event<'_>) {
        match event {
            Event::Discarded { stage, dir } => {
                self.print(&format!(
                    "{stage}: discarded interrupted output {}",
                    dir.display()
                ));
            }
            Event::Running {
                stage,
                dir,
                index,
                total,
                estimate,
            } => self.start(stage, dir, index, total, estimate),
            Event::Finished(r) => {
                if r.stage == "frames" {
                    self.frames = r.metrics.get("kept").map(|&n| n as usize);
                }
                let current = self.stop();
                let seconds = current.as_ref().map(|c| c.started.elapsed().as_secs_f64());
                self.rows.push((r.stage, seconds.filter(|_| !r.cached)));
                let index = self
                    .plan
                    .iter()
                    .position(|&s| s == r.stage)
                    .map_or(0, |i| i + 1);
                let label = format!("[{index}/{}] {}", self.plan.len(), r.stage);
                let line = match (&self.live, r.cached, current) {
                    (Some(_), true, _) => format!("· {:<6}   cached", r.stage),
                    (Some(_), false, c) => {
                        let usual = c
                            .and_then(|c| c.estimate)
                            .map(|e| format!("   ({e})"))
                            .unwrap_or_default();
                        format!(
                            "✓ {:<6}   {}{usual}",
                            r.stage,
                            elapsed(seconds.unwrap_or(0.0))
                        )
                    }
                    (None, true, _) => format!("{label}: cached"),
                    (None, false, _) => {
                        format!("{label}: done in {}", elapsed(seconds.unwrap_or(0.0)))
                    }
                };
                self.print(&line);
            }
            Event::Warning { stage, message } => {
                self.print(&format!("{stage}: warning: {message}"));
            }
        }
    }

    fn start(
        &mut self,
        stage: &'static str,
        dir: &Path,
        index: usize,
        total: usize,
        estimate: Option<Estimate>,
    ) {
        self.stop();
        self.ran.insert(stage);
        let label = format!("[{index}/{total}] {stage}");
        let Some(planned) = self.planned.get(stage) else {
            return;
        };
        let after: Vec<Option<Estimate>> = will_run(&self.plan, &self.planned, &self.ran, stage)
            .iter()
            .map(|s| self.planned[s].estimate)
            .collect();
        let bars = self.live.as_ref().map(|multi| {
            ["{spinner:.cyan} {wide_msg}", "  {wide_msg}"].map(|template| {
                let style = ProgressStyle::with_template(template).expect("valid template");
                multi.add(ProgressBar::new_spinner().with_style(style))
            })
        });
        let sink = match &bars {
            Some([stage, total]) => {
                stage.enable_steady_tick(Duration::from_millis(100));
                Sink::Live {
                    stage: stage.clone(),
                    total: total.clone(),
                }
            }
            None => {
                let usual = estimate.map(|e| format!(" ({e})")).unwrap_or_default();
                eprintln!("{label}: running{usual} in {}", dir.display());
                Sink::Plain
            }
        };
        let started = Instant::now();
        let watch = Watch {
            stage: planned.stage,
            logs: dir.join("logs"),
            steps: planned.steps.clone(),
            frames: self.frames,
            label,
            estimate,
            after,
            run_started: self.started,
            started,
            sink,
        };
        let (stop, stopped) = mpsc::channel();
        let watcher = std::thread::spawn(move || watch.run(&stopped));
        self.current = Some(Current {
            started,
            bars,
            estimate,
            stop,
            watcher: Some(watcher),
        });
    }

    /// Stop the watcher of the running stage and hide its lines.
    fn stop(&mut self) -> Option<Current> {
        let mut current = self.current.take()?;
        let _ = current.stop.send(());
        if let Some(watcher) = current.watcher.take() {
            let _ = watcher.join();
        }
        if let (Some(multi), Some(bars)) = (&self.live, &current.bars) {
            for bar in bars {
                bar.finish_and_clear();
                multi.remove(bar);
            }
        }
        Some(current)
    }

    /// The end-of-run table: each stage, whether it ran, its time, the total.
    pub fn summary(&mut self) {
        self.stop();
        let mut lines = vec![format!("{:<8}{:<8}{}", "stage", "status", "time")];
        for (stage, seconds) in &self.rows {
            lines.push(match seconds {
                Some(s) => format!("{stage:<8}{:<8}{}", "ran", elapsed(*s)),
                None => format!("{stage:<8}{:<8}", "cached"),
            });
        }
        lines.push(format!(
            "{:<16}{}",
            "total",
            elapsed(self.started.elapsed().as_secs_f64())
        ));
        for line in lines {
            self.print(line.trim_end());
        }
    }
}

impl Drop for Reporter {
    fn drop(&mut self) {
        self.stop();
    }
}

/// The stages after `current` that will run too: those depending on a stage
/// that ran. A changed fingerprint changes every dependent's.
fn will_run(
    plan: &[&'static str],
    planned: &BTreeMap<&'static str, Planned>,
    ran: &BTreeSet<&'static str>,
    current: &str,
) -> Vec<&'static str> {
    let mut running: BTreeSet<&str> = ran.iter().copied().collect();
    let after = plan.iter().skip_while(|&&s| s != current).skip(1);
    let mut out = Vec::new();
    for &stage in after {
        let Some(p) = planned.get(stage) else {
            continue;
        };
        if p.stage.deps().iter().any(|d| running.contains(d)) {
            running.insert(stage);
            out.push(stage);
        }
    }
    out
}

enum Sink {
    Live {
        stage: ProgressBar,
        total: ProgressBar,
    },
    Plain,
}

/// What the watcher thread needs about the running stage.
struct Watch {
    stage: SyncStage,
    logs: PathBuf,
    steps: Vec<String>,
    frames: Option<usize>,
    label: String,
    estimate: Option<Estimate>,
    /// Estimates of the stages expected to run after this one.
    after: Vec<Option<Estimate>>,
    run_started: Instant,
    started: Instant,
    sink: Sink,
}

/// The running stage as its newest log shows it.
#[derive(Debug, Default, PartialEq)]
struct Snapshot {
    /// Position from 1 and name.
    step: Option<(usize, String)>,
    fraction: Option<f64>,
    line: String,
}

impl Watch {
    fn run(&self, stopped: &mpsc::Receiver<()>) {
        let mut step = None;
        let mut beat = Instant::now();
        loop {
            let snap = self.snapshot();
            let seconds = self.started.elapsed().as_secs_f64();
            match &self.sink {
                Sink::Live { stage, total } => {
                    stage.set_message(stage_line(
                        &self.label,
                        &snap,
                        self.steps.len(),
                        seconds,
                        self.estimate,
                    ));
                    let left = Left::of(self.estimate, seconds, &self.after);
                    total.set_message(total_line(self.run_started.elapsed().as_secs_f64(), left));
                }
                Sink::Plain => {
                    if let Some((i, name)) = &snap.step
                        && step.as_ref() != Some(name)
                    {
                        eprintln!("{}: step {i}/{} {name}", self.label, self.steps.len());
                        step = Some(name.clone());
                    }
                    if beat.elapsed() >= HEARTBEAT {
                        eprintln!("{}", heartbeat(&self.label, &snap, seconds));
                        beat = Instant::now();
                    }
                }
            }
            match stopped.recv_timeout(POLL) {
                Err(RecvTimeoutError::Timeout) => {}
                _ => return,
            }
        }
    }

    fn snapshot(&self) -> Snapshot {
        let Some((index, path)) = newest_log(&self.logs, &self.steps) else {
            return Snapshot::default();
        };
        let bytes = std::fs::read(&path).unwrap_or_default();
        let text = String::from_utf8_lossy(&bytes);
        let name = self.steps[index].clone();
        Snapshot {
            fraction: self.stage.progress(&name, &text, self.frames),
            line: last_line(&text),
            step: Some((index + 1, name)),
        }
    }
}

/// The newest `<step>.log` in `logs` for one of `steps`, with its index.
fn newest_log(logs: &Path, steps: &[String]) -> Option<(usize, PathBuf)> {
    let entries = std::fs::read_dir(logs).ok()?;
    entries
        .filter_map(|e| e.ok())
        .filter_map(|e| {
            let path = e.path();
            let stem = path.file_stem()?.to_str()?;
            let index = steps.iter().position(|s| s == stem)?;
            (path.extension()? == "log").then_some(())?;
            let modified = e
                .metadata()
                .and_then(|m| m.modified())
                .unwrap_or(SystemTime::UNIX_EPOCH);
            Some((modified, index, path))
        })
        .max_by_key(|(modified, index, _)| (*modified, *index))
        .map(|(_, index, path)| (index, path))
}

/// The last non-empty line of tool output, without colour codes. Progress
/// lines are redrawn with carriage returns, so those end lines too.
fn last_line(text: &str) -> String {
    text.rsplit(['\n', '\r'])
        .map(|line| strip_ansi(line).trim().to_owned())
        .find(|line| !line.is_empty())
        .unwrap_or_default()
}

/// `text` without ANSI escape sequences (CSI `ESC [ … letter` and two-byte ones).
fn strip_ansi(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c != '\u{1b}' {
            out.push(c);
            continue;
        }
        if chars.next() == Some('[') {
            for c in chars.by_ref() {
                if ('@'..='~').contains(&c) {
                    break;
                }
            }
        }
    }
    out
}

fn percent(fraction: Option<f64>) -> String {
    fraction.map_or_else(String::new, |f| format!("{:.0}%", f * 100.0))
}

/// `[4/7] dense   DensifyPointCloud 3/5  38%  4m12s · usual 10m–26m  <last line>`
fn stage_line(
    label: &str,
    snap: &Snapshot,
    steps: usize,
    seconds: f64,
    estimate: Option<Estimate>,
) -> String {
    let mut parts = vec![label.to_owned()];
    if let Some((i, name)) = &snap.step {
        parts.push(format!("{name} {i}/{steps}"));
    }
    if snap.fraction.is_some() {
        parts.push(percent(snap.fraction));
    }
    let usual = estimate.map(|e| format!(" · {e}")).unwrap_or_default();
    parts.push(format!("{}{usual}", elapsed(seconds)));
    if !snap.line.is_empty() {
        parts.push(snap.line.clone());
    }
    parts.join("  ")
}

/// `[4/7] dense: still running, 5m00s, 38% — <last line>`
fn heartbeat(label: &str, snap: &Snapshot, seconds: f64) -> String {
    let mut line = format!("{label}: still running, {}", elapsed(seconds));
    if snap.fraction.is_some() {
        line += &format!(", {}", percent(snap.fraction));
    }
    if !snap.line.is_empty() {
        line += &format!(" — {}", snap.line);
    }
    line
}

/// Time left in the run: the rest of the current stage's range and the
/// ranges of the stages expected after it.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Left {
    min: f64,
    max: f64,
    /// A stage without history: the range is a lower bound.
    unknown: bool,
}

impl Left {
    fn of(current: Option<Estimate>, seconds: f64, after: &[Option<Estimate>]) -> Self {
        let mut left = Left {
            min: 0.0,
            max: 0.0,
            unknown: false,
        };
        let rest = current.map(|e| Estimate {
            min: (e.min - seconds).max(0.0),
            max: (e.max - seconds).max(0.0),
            n: e.n,
        });
        for estimate in std::iter::once(rest).chain(after.iter().copied()) {
            match estimate {
                Some(e) => {
                    left.min += e.min;
                    left.max += e.max;
                }
                None => left.unknown = true,
            }
        }
        left
    }
}

/// `total 18m40s · ~20m–35m left`
fn total_line(seconds: f64, left: Left) -> String {
    let (min, max) = (rough(left.min), rough(left.max));
    let left = match (left.unknown, min == max) {
        _ if left.max == 0.0 => String::new(),
        (true, _) => format!(" · ≥{min} left"),
        (false, true) => format!(" · ~{max} left"),
        (false, false) => format!(" · ~{min}–{max} left"),
    };
    format!("total {}{left}", elapsed(seconds))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn est(min: f64, max: f64) -> Option<Estimate> {
        Some(Estimate { min, max, n: 2 })
    }

    #[test]
    fn last_line_takes_the_latest_redraw_without_colour() {
        let log = "Loading weights:   0%|          | 0/802\rLoading weights: 100%|█| 802/802\n\
                   [transformers] \u{1b}[1mOneFormer LOAD REPORT\u{1b}[0m from: /opt\n\n";
        assert_eq!(
            last_line(log),
            "[transformers] OneFormer LOAD REPORT from: /opt"
        );
        let redraw = "Estimated depth-maps 46 (38.02%, 8s, ETA 14s)...\rEstimated depth-maps 47 (38.84%, 9s, ETA 14s)...   \r";
        assert_eq!(
            last_line(redraw),
            "Estimated depth-maps 47 (38.84%, 9s, ETA 14s)..."
        );
        assert_eq!(last_line(""), "");
    }

    #[test]
    fn newest_log_is_the_current_step() {
        let dir = tempfile::tempdir().unwrap();
        let steps: Vec<String> = ["a", "b", "c"].map(String::from).to_vec();
        assert_eq!(newest_log(dir.path(), &steps), None);
        std::fs::write(dir.path().join("a.log"), "").unwrap();
        std::fs::write(dir.path().join("other.log"), "").unwrap();
        assert_eq!(newest_log(dir.path(), &steps).unwrap().0, 0);
        // Equal modification times (coarse clocks) go to the later step.
        std::fs::write(dir.path().join("b.log"), "").unwrap();
        let old = SystemTime::UNIX_EPOCH + Duration::from_secs(1);
        std::fs::File::options()
            .write(true)
            .open(dir.path().join("a.log"))
            .unwrap()
            .set_modified(old)
            .unwrap();
        assert_eq!(
            newest_log(dir.path(), &steps),
            Some((1, dir.path().join("b.log")))
        );
        assert_eq!(newest_log(&dir.path().join("absent"), &steps), None);
    }

    #[test]
    fn lines_show_step_percent_time_and_estimate() {
        let snap = Snapshot {
            step: Some((3, "DensifyPointCloud".into())),
            fraction: Some(0.3884),
            line: "Estimated depth-maps 47 (38.84%, 9s, ETA 14s)...".into(),
        };
        assert_eq!(
            stage_line("[4/7] dense", &snap, 5, 252.0, est(600.0, 1560.0)),
            "[4/7] dense  DensifyPointCloud 3/5  39%  4m12s · usual 10m–26m  \
             Estimated depth-maps 47 (38.84%, 9s, ETA 14s)..."
        );
        assert_eq!(
            stage_line("[6/7] post", &Snapshot::default(), 0, 3.0, None),
            "[6/7] post  3s"
        );
        assert_eq!(
            heartbeat("[4/7] dense", &snap, 300.0),
            "[4/7] dense: still running, 5m00s, 39% — Estimated depth-maps 47 (38.84%, 9s, ETA 14s)..."
        );
        assert_eq!(
            heartbeat("[6/7] post", &Snapshot::default(), 60.0),
            "[6/7] post: still running, 1m00s"
        );
    }

    #[test]
    fn stages_after_the_current_one_run_when_they_depend_on_one_that_ran() {
        let planned = |stages: &[SyncStage]| -> BTreeMap<&'static str, Planned> {
            stages
                .iter()
                .map(|&stage| {
                    let p = Planned {
                        stage,
                        steps: Vec::new(),
                        estimate: None,
                    };
                    (stage.name(), p)
                })
                .collect()
        };
        let stages: [SyncStage; 6] = [
            &shoreline_frames::Frames,
            &shoreline_masks::Masks,
            &shoreline_sfm::Sfm,
            &shoreline_dense::Dense,
            &shoreline_splat::Splat,
            &shoreline_post::Post,
        ];
        let plan: Vec<_> = stages.iter().map(|s| s.name()).collect();
        let p = planned(&stages);
        let ran = |names: &[&'static str]| names.iter().copied().collect();
        assert_eq!(
            will_run(&plan, &p, &ran(&["masks"]), "masks"),
            ["sfm", "dense", "splat", "post"]
        );
        // splat does not depend on dense; post depends on both.
        assert_eq!(will_run(&plan, &p, &ran(&["dense"]), "dense"), ["post"]);
        assert!(will_run(&plan, &p, &ran(&["post"]), "post").is_empty());
    }

    #[test]
    fn time_left_sums_the_rest_of_the_current_stage_and_those_after() {
        let left = Left::of(
            est(600.0, 1560.0),
            252.0,
            &[est(1000.0, 1100.0), est(5.0, 5.0)],
        );
        assert_eq!((left.min, left.max, left.unknown), (1353.0, 2413.0, false));
        assert_eq!(total_line(1120.0, left), "total 18m40s · ~23m–40m left");
        // Past the usual maximum the current stage adds nothing.
        let over = Left::of(est(600.0, 700.0), 900.0, &[]);
        assert_eq!(total_line(900.0, over), "total 15m00s");
        let unknown = Left::of(None, 10.0, &[est(60.0, 120.0)]);
        assert_eq!(total_line(10.0, unknown), "total 10s · ≥1m left");
        assert_eq!(total_line(10.0, Left::of(None, 10.0, &[])), "total 10s");
    }
}
