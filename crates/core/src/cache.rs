//! Stage output cache (ADR-0004).
//!
//! Each stage output lives in `work/<dataset>/<stage>/<fingerprint>/`. Its
//! `manifest.json` is written last, atomically, and marks the output
//! complete: a directory without one is an interrupted run.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::fingerprint::Fingerprint;
use crate::stage::Metrics;
use crate::tools::ToolRecord;

pub const MANIFEST_FILE: &str = "manifest.json";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Upstream {
    pub stage: String,
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

/// Write `manifest` into `dir` atomically (temporary file, then rename).
pub fn write_manifest(dir: &Path, manifest: &Manifest) -> Result<()> {
    // serde_json writes NaN and infinities as null, which would not read back.
    if let Some((name, _)) = manifest.metrics.iter().find(|(_, v)| !v.is_finite()) {
        return Err(Error::Cache(format!(
            "{}: metric {name} is not finite",
            manifest.stage
        )));
    }
    let json = serde_json::to_vec_pretty(manifest).expect("manifest serialises to JSON");
    let tmp = dir.join(format!(".{MANIFEST_FILE}.tmp"));
    let path = dir.join(MANIFEST_FILE);
    std::fs::write(&tmp, json).map_err(|source| Error::Io {
        path: tmp.clone(),
        source,
    })?;
    std::fs::rename(&tmp, &path).map_err(|source| Error::Io { path, source })
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
            tools: vec![ToolRecord {
                tool: "ffmpeg".into(),
                runtime: Runtime::Native,
                image: None,
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
        write_manifest(dir.path(), &m).unwrap();
        assert_eq!(lookup(dir.path(), &fp(1)).unwrap(), Some(m));
        let names: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(names, [MANIFEST_FILE], "no temporary file left behind");
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
        write_manifest(dir.path(), &manifest(fp(1))).unwrap();
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
        let err = write_manifest(dir.path(), &m).unwrap_err();
        assert!(
            matches!(&err, Error::Cache(msg) if msg.contains("ratio")),
            "{err}"
        );
        assert!(!dir.path().join(MANIFEST_FILE).exists());
    }
}
