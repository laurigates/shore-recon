//! Stage output cache (ADR-0004).
//!
//! Each stage output lives in `work/<dataset>/<stage>/<fingerprint>/`. Its
//! `manifest.json` is written last, atomically and after the outputs are
//! flushed to disk, and marks the output complete: a directory without one
//! is an interrupted run.

use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result, io_error};
use crate::fingerprint::Fingerprint;
use crate::stage::Metrics;
use crate::tools::ToolRecord;

pub const MANIFEST_FILE: &str = "manifest.json";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Upstream {
    pub stage: String,
    pub fingerprint: Fingerprint,
}

/// The dataset input a root stage read, identified by content.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Source {
    pub path: PathBuf,
    pub fingerprint: Fingerprint,
}

/// Lineage and results of one stage output.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub stage: String,
    pub fingerprint: Fingerprint,
    pub config: serde_json::Value,
    pub version: String,
    pub upstream: Vec<Upstream>,
    /// Set for stages without dependencies, which read the dataset input.
    pub source: Option<Source>,
    /// Tool logs are relative to the output directory.
    pub tools: Vec<ToolRecord>,
    pub metrics: Metrics,
    pub created_unix: u64,
}

/// The manifest in `dir`, if the output there is complete. A manifest for a
/// different fingerprint, or one that does not parse, is an error.
pub fn lookup(dir: &Path, fp: &Fingerprint) -> Result<Option<Manifest>> {
    let path = dir.join(MANIFEST_FILE);
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(source) => return Err(Error::Io { path, source }),
    };
    let manifest: Manifest = serde_json::from_str(&text)
        .map_err(|e| Error::Cache(format!("{}: {e}", path.display())))?;
    if manifest.fingerprint != *fp {
        return Err(Error::Cache(format!(
            "{} is for {}, expected {fp}",
            path.display(),
            manifest.fingerprint
        )));
    }
    Ok(Some(manifest))
}

/// Mark the output in `dir` complete: flush its files, then write
/// `manifest` through a flushed temporary file and a rename, and flush the
/// directory entries. A crash can then leave no manifest, but never a
/// truncated one or one over unwritten outputs.
pub fn publish(dir: &Path, manifest: &Manifest) -> Result<()> {
    // serde_json writes NaN and infinities as null, which would not read back.
    if let Some((name, _)) = manifest.metrics.iter().find(|(_, v)| !v.is_finite()) {
        return Err(Error::Cache(format!(
            "{}: metric {name} is not finite",
            manifest.stage
        )));
    }
    sync_tree(dir)?;
    let json = serde_json::to_vec_pretty(manifest).expect("manifest serialises to JSON");
    let tmp = dir.join(format!(".{MANIFEST_FILE}.tmp"));
    let path = dir.join(MANIFEST_FILE);
    let mut file = File::create(&tmp).map_err(io_error(&tmp))?;
    file.write_all(&json)
        .and_then(|()| file.sync_all())
        .map_err(io_error(&tmp))?;
    std::fs::rename(&tmp, &path).map_err(io_error(&path))?;
    sync_dir(dir)?;
    match dir.parent() {
        Some(parent) => sync_dir(parent),
        None => Ok(()),
    }
}

/// Flush every regular file and directory below `dir`, then `dir` itself.
fn sync_tree(dir: &Path) -> Result<()> {
    for entry in std::fs::read_dir(dir).map_err(io_error(dir))? {
        let entry = entry.map_err(io_error(dir))?;
        let path = entry.path();
        let kind = entry.file_type().map_err(io_error(&path))?;
        if kind.is_dir() {
            sync_tree(&path)?;
        } else if kind.is_file() {
            File::open(&path)
                .and_then(|f| f.sync_all())
                .map_err(io_error(&path))?;
        }
    }
    sync_dir(dir)
}

fn sync_dir(dir: &Path) -> Result<()> {
    File::open(dir)
        .and_then(|f| f.sync_all())
        .map_err(io_error(dir))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::Runtime;

    fn manifest(fp: Fingerprint) -> Manifest {
        Manifest {
            stage: "frames".into(),
            fingerprint: fp,
            config: serde_json::json!({"fps": 2.0}),
            version: "frames/1 ffmpeg 9".into(),
            upstream: vec![Upstream {
                stage: "ingest".into(),
                fingerprint: Fingerprint::compute("ingest", &0, &[], ""),
            }],
            source: Some(Source {
                path: "data/raw/x.mp4".into(),
                fingerprint: Fingerprint::compute("source", &0, &[], ""),
            }),
            tools: vec![ToolRecord {
                tool: "ffmpeg".into(),
                runtime: Runtime::Native,
                image: None,
                host: None,
                argv: vec!["ffmpeg".into(), "-version".into()],
                seconds: 0.5,
                log: "logs/ffmpeg.log".into(),
            }],
            metrics: [("kept".to_owned(), 121.0)].into(),
            created_unix: 1_761_300_000,
        }
    }

    fn fp(n: u32) -> Fingerprint {
        Fingerprint::compute("frames", &n, &[], "")
    }

    #[test]
    fn manifest_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let m = manifest(fp(1));
        publish(dir.path(), &m).unwrap();
        assert_eq!(lookup(dir.path(), &fp(1)).unwrap(), Some(m));
        let names: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(names, [MANIFEST_FILE], "no temporary file left behind");
    }

    #[test]
    fn publish_flushes_nested_outputs() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("logs")).unwrap();
        std::fs::write(dir.path().join("logs/ffmpeg.log"), "ok").unwrap();
        publish(dir.path(), &manifest(fp(1))).unwrap();
        assert!(lookup(dir.path(), &fp(1)).unwrap().is_some());
    }

    #[test]
    fn missing_dir_or_manifest_is_none() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(lookup(&dir.path().join("absent"), &fp(1)).unwrap(), None);
        std::fs::write(dir.path().join("partial.jpg"), "").unwrap();
        assert_eq!(lookup(dir.path(), &fp(1)).unwrap(), None);
    }

    #[test]
    fn manifest_for_another_fingerprint_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        publish(dir.path(), &manifest(fp(1))).unwrap();
        let err = lookup(dir.path(), &fp(2)).unwrap_err();
        assert!(
            matches!(&err, Error::Cache(m) if m.contains(&fp(2).to_string())),
            "{err}"
        );
    }

    #[test]
    fn unparsable_manifest_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(MANIFEST_FILE), "{").unwrap();
        assert!(matches!(lookup(dir.path(), &fp(1)), Err(Error::Cache(_))));
    }

    #[test]
    fn non_finite_metrics_are_rejected_before_writing() {
        let dir = tempfile::tempdir().unwrap();
        let mut m = manifest(fp(1));
        m.metrics.insert("ratio".into(), f64::NAN);
        let err = publish(dir.path(), &m).unwrap_err();
        assert!(
            matches!(&err, Error::Cache(msg) if msg.contains("ratio")),
            "{err}"
        );
        assert!(!dir.path().join(MANIFEST_FILE).exists());
    }
}
