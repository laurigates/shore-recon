//! Stage durations from earlier runs, for estimates (ADR-0018).
//!
//! History is the manifests already in the cache: every
//! `<work root>/<dataset>/<stage>/<fingerprint>/manifest.json`, across
//! datasets. It is advisory, so a manifest that cannot be read is skipped,
//! never an error.

use std::fmt;
use std::path::Path;

use serde::Deserialize;

use crate::cache::MANIFEST_FILE;

/// Just the fields history needs, so manifests of other versions still count.
#[derive(Deserialize)]
struct Timing {
    #[serde(default)]
    seconds: Option<f64>,
    #[serde(default)]
    tools: Vec<ToolTiming>,
    /// Stage version recorded in the manifest (ADR-0004); absent in the
    /// oldest manifests, which then match no specific version.
    #[serde(default)]
    version: Option<String>,
}

#[derive(Deserialize)]
struct ToolTiming {
    seconds: f64,
}

/// Seconds each completed run of `stage` took, under every dataset in
/// `work_root`. A manifest without a wall time counts its tools' time.
///
/// When `version` is `Some`, only runs whose recorded stage version matches
/// are kept; if none match, every run is kept as a fallback (history is
/// advisory). `None` keeps every run.
pub fn durations(work_root: &Path, stage: &str, version: Option<&str>) -> Vec<f64> {
    let mut all = Vec::new();
    let mut matching = Vec::new();
    for dataset in subdirs(work_root) {
        for output in subdirs(&dataset.join(stage)) {
            let Ok(text) = std::fs::read_to_string(output.join(MANIFEST_FILE)) else {
                continue;
            };
            let Ok(t) = serde_json::from_str::<Timing>(&text) else {
                continue;
            };
            let seconds = t
                .seconds
                .unwrap_or_else(|| t.tools.iter().map(|tool| tool.seconds).sum());
            if !(seconds.is_finite() && seconds > 0.0) {
                continue;
            }
            all.push(seconds);
            if version.is_none_or(|v| t.version.as_deref() == Some(v)) {
                matching.push(seconds);
            }
        }
    }
    match version {
        Some(_) if !matching.is_empty() => matching,
        _ => all,
    }
}

fn subdirs(dir: &Path) -> Vec<std::path::PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut dirs: Vec<_> = entries
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .map(|e| e.path())
        .collect();
    dirs.sort();
    dirs
}

/// The range of earlier durations of one stage.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Estimate {
    pub min: f64,
    pub max: f64,
    /// Number of runs it is based on.
    pub n: usize,
}

impl Estimate {
    pub fn from_durations(durations: &[f64]) -> Option<Self> {
        let min = durations.iter().copied().reduce(f64::min)?;
        let max = durations.iter().copied().reduce(f64::max)?;
        Some(Self {
            min,
            max,
            n: durations.len(),
        })
    }

    /// From [`durations`] of `stage` under `work_root`.
    pub fn of(work_root: &Path, stage: &str, version: Option<&str>) -> Option<Self> {
        Self::from_durations(&durations(work_root, stage, version))
    }
}

/// `usual 10m–26m` from two or more runs, `last 7m` from one.
impl fmt::Display for Estimate {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (min, max) = (rough(self.min), rough(self.max));
        match (self.n, min == max) {
            (1, _) => write!(f, "last {max}"),
            (_, true) => write!(f, "usual {max}"),
            _ => write!(f, "usual {min}–{max}"),
        }
    }
}

/// `7s`, `7m02s`, `1h03m`: for measured times.
pub fn elapsed(seconds: f64) -> String {
    let s = seconds.max(0.0).round() as u64;
    match s {
        0..60 => format!("{s}s"),
        60..3600 => format!("{}m{:02}s", s / 60, s % 60),
        _ => format!("{}h{:02}m", s / 3600, s % 3600 / 60),
    }
}

/// `7s`, `7m`, `1h03m`: for estimates, which are not that precise.
pub fn rough(seconds: f64) -> String {
    let s = seconds.max(0.0).round() as u64;
    match s {
        0..60 => format!("{s}s"),
        60..3600 => format!("{}m", (s + 30) / 60),
        _ => format!("{}h{:02}m", s / 3600, s % 3600 / 60),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(root: &Path, rel: &str, json: &str) {
        let dir = root.join(rel);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(MANIFEST_FILE), json).unwrap();
    }

    #[test]
    fn durations_span_datasets_and_fall_back_to_tool_time() {
        let root = tempfile::tempdir().unwrap();
        let r = root.path();
        write(r, "pier/dense/aaa", r#"{"seconds": 600.0, "tools": []}"#);
        write(
            r,
            "shore/dense/bbb",
            r#"{"stage": "dense", "tools": [{"seconds": 1000.0}, {"seconds": 560.0}]}"#,
        );
        write(r, "pier/sfm/ccc", r#"{"seconds": 200.0}"#);
        write(r, "pier/dense/broken", "{");
        std::fs::create_dir_all(r.join("pier/dense/interrupted")).unwrap();
        std::fs::write(r.join("stray-file"), "").unwrap();

        assert_eq!(durations(r, "dense", None), [600.0, 1560.0]);
        assert_eq!(durations(r, "sfm", None), [200.0]);
        assert!(durations(r, "splat", None).is_empty());
        assert!(durations(&r.join("absent"), "dense", None).is_empty());
    }

    #[test]
    fn durations_keep_only_matching_stage_version() {
        let root = tempfile::tempdir().unwrap();
        let r = root.path();
        write(r, "a/dense/x", r#"{"version":"v1","seconds":100.0}"#);
        write(r, "b/dense/y", r#"{"version":"v2","seconds":200.0}"#);
        assert_eq!(durations(r, "dense", Some("v1")), [100.0]);
        assert_eq!(durations(r, "dense", Some("v2")), [200.0]);
        // no matching version: fall back to every run
        assert_eq!(durations(r, "dense", Some("v3")), [100.0, 200.0]);
    }

    #[test]
    fn estimate_of_filters_by_stage_version() {
        let root = tempfile::tempdir().unwrap();
        let r = root.path();
        write(r, "a/dense/x", r#"{"version":"v1","seconds":100.0}"#);
        write(r, "b/dense/y", r#"{"version":"v2","seconds":200.0}"#);
        // matching version: only that run contributes
        let e = Estimate::of(r, "dense", Some("v1")).unwrap();
        assert_eq!((e.min, e.max, e.n), (100.0, 100.0, 1));
        // non-matching version: fallback to every run
        let e = Estimate::of(r, "dense", Some("v3")).unwrap();
        assert_eq!(e.n, 2);
        // None keeps every run
        let e = Estimate::of(r, "dense", None).unwrap();
        assert_eq!(e.n, 2);
    }

    #[test]
    fn estimates_read_as_a_range_or_the_last_run() {
        assert_eq!(Estimate::from_durations(&[]), None);
        let e = Estimate::from_durations(&[1560.0, 600.0, 900.0]).unwrap();
        assert_eq!((e.min, e.max, e.n), (600.0, 1560.0, 3));
        assert_eq!(e.to_string(), "usual 10m–26m");
        assert_eq!(
            Estimate::from_durations(&[422.0]).unwrap().to_string(),
            "last 7m"
        );
        assert_eq!(
            Estimate::from_durations(&[7.2, 6.9]).unwrap().to_string(),
            "usual 7s"
        );
    }

    #[test]
    fn times_format_by_magnitude() {
        assert_eq!(elapsed(7.4), "7s");
        assert_eq!(elapsed(422.0), "7m02s");
        assert_eq!(elapsed(3780.0), "1h03m");
        assert_eq!(rough(422.0), "7m");
        assert_eq!(rough(450.0), "8m");
        assert_eq!(rough(59.0), "59s");
        assert_eq!(rough(-3.0), "0s");
    }
}
