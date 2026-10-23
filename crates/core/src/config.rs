//! Run configuration.
//!
//! A dataset config is deep-merged over `defaults.toml` from the same
//! directory, when present. Tables merge recursively; any other value in
//! the dataset file replaces the default.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use toml::{Table, Value};

use crate::error::{Error, Result};

pub const DEFAULTS_FILE: &str = "defaults.toml";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub dataset: Dataset,
    pub frames: Frames,
    pub masks: Masks,
    pub sfm: Sfm,
    pub dense: Dense,
    pub splat: Splat,
    pub post: Post,
    pub gates: Gates,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Dataset {
    pub name: String,
    pub input: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Frames {
    pub fps: f64,
    pub blur_threshold: f64,
    pub max_long_edge: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Masks {
    pub backend: String,
    pub model: Option<PathBuf>,
    pub classes: Vec<String>,
    pub dilate_px: u32,
    pub temporal: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Matcher {
    Sequential,
    Exhaustive,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Sfm {
    pub backend: String,
    pub matcher: Matcher,
    pub single_camera: bool,
    pub max_features: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Dense {
    pub backend: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Splat {
    pub backend: String,
    pub steps: u32,
}

/// Water plane source: fitted from sparse points or a fixed height.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum WaterPlane {
    Fit(FitTag),
    Height(f64),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FitTag {
    Fit,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Post {
    pub water_plane: WaterPlane,
    pub min_component: f64,
    pub target_faces: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Gates {
    pub min_registered_ratio: f64,
    pub max_reproj_error_px: f64,
    pub min_track_length: f64,
    pub max_masked_fraction: f64,
}

impl Config {
    /// Load a dataset config, merged over `defaults.toml` beside it.
    pub fn load(path: &Path) -> Result<Self> {
        let mut table = read_table(path)?;
        let defaults = path
            .parent()
            .map(|dir| dir.join(DEFAULTS_FILE))
            .filter(|p| p.is_file() && !same_file(p, path));
        if let Some(defaults) = defaults {
            let mut base = read_table(&defaults)?;
            merge(&mut base, table);
            table = base;
        }
        Self::from_table(table)
    }

    pub fn from_table(table: Table) -> Result<Self> {
        let config: Self = Value::Table(table)
            .try_into()
            .map_err(|e: toml::de::Error| Error::Config(e.message().to_owned()))?;
        config.validate()?;
        Ok(config)
    }

    fn validate(&self) -> Result<()> {
        let invalid = |msg: &str| Err(Error::Config(msg.to_owned()));
        if self.dataset.name.is_empty() {
            return invalid("dataset.name must not be empty");
        }
        if self.frames.fps <= 0.0 {
            return invalid("frames.fps must be positive");
        }
        if !(0.0..=1.0).contains(&self.post.min_component) {
            return invalid("post.min_component must be within [0, 1]");
        }
        if !(0.0..=1.0).contains(&self.gates.min_registered_ratio) {
            return invalid("gates.min_registered_ratio must be within [0, 1]");
        }
        if !(0.0..=1.0).contains(&self.gates.max_masked_fraction) {
            return invalid("gates.max_masked_fraction must be within [0, 1]");
        }
        Ok(())
    }
}

/// Deep-merge `overlay` into `base`. Nested tables merge; other values replace.
pub fn merge(base: &mut Table, overlay: Table) {
    for (key, value) in overlay {
        match (base.get_mut(&key), value) {
            (Some(Value::Table(b)), Value::Table(o)) => merge(b, o),
            (_, v) => {
                base.insert(key, v);
            }
        }
    }
}

fn read_table(path: &Path) -> Result<Table> {
    let text = std::fs::read_to_string(path).map_err(|source| Error::Io {
        path: path.to_owned(),
        source,
    })?;
    text.parse::<Table>().map_err(|source| Error::Parse {
        path: path.to_owned(),
        source: Box::new(source),
    })
}

fn same_file(a: &Path, b: &Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn table(s: &str) -> Table {
        s.parse().unwrap()
    }

    fn repo_configs() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../configs")
    }

    #[test]
    fn merge_overrides_scalars_and_recurses_into_tables() {
        let mut base = table("a = 1\n[t]\nx = 1\ny = [1, 2]\n");
        merge(&mut base, table("[t]\ny = [3]\nz = true\n"));
        assert_eq!(base, table("a = 1\n[t]\nx = 1\ny = [3]\nz = true\n"));
    }

    #[test]
    fn merge_replaces_table_with_scalar() {
        let mut base = table("[t]\nx = 1\n");
        merge(&mut base, table("t = 2\n"));
        assert_eq!(base, table("t = 2\n"));
    }

    #[test]
    fn water_plane_accepts_fit_or_height() {
        let fit: Post =
            toml::from_str("water_plane = \"fit\"\nmin_component = 0.01\ntarget_faces = 1\n")
                .unwrap();
        assert_eq!(fit.water_plane, WaterPlane::Fit(FitTag::Fit));
        let fixed: Post =
            toml::from_str("water_plane = -0.5\nmin_component = 0.01\ntarget_faces = 1\n").unwrap();
        assert_eq!(fixed.water_plane, WaterPlane::Height(-0.5));
        assert!(
            toml::from_str::<Post>(
                "water_plane = \"auto\"\nmin_component = 0.01\ntarget_faces = 1\n"
            )
            .is_err()
        );
    }

    #[test]
    fn repo_dataset_configs_load() {
        for name in ["reposalmi-laajasalo", "reposalmi-vartiosaari"] {
            let cfg = Config::load(&repo_configs().join(format!("{name}.toml"))).unwrap();
            assert_eq!(cfg.dataset.name, name);
        }
    }

    #[test]
    fn dataset_overrides_defaults() {
        let cfg = Config::load(&repo_configs().join("reposalmi-vartiosaari.toml")).unwrap();
        let defaults = read_table(&repo_configs().join(DEFAULTS_FILE)).unwrap();
        assert_eq!(
            cfg.masks.dilate_px as i64,
            defaults["masks"]["dilate_px"].as_integer().unwrap()
        );
        assert_eq!(cfg.sfm.matcher, Matcher::Exhaustive);
    }

    #[test]
    fn unknown_keys_are_rejected() {
        let mut t = read_table(&repo_configs().join(DEFAULTS_FILE)).unwrap();
        merge(
            &mut t,
            table("[dataset]\nname = \"x\"\ninput = \"x\"\n[sfm]\nmax_featres = 1\n"),
        );
        let err = Config::from_table(t).unwrap_err().to_string();
        assert!(err.contains("max_featres"), "{err}");
    }

    #[test]
    fn validation_rejects_out_of_range_values() {
        let mut t = read_table(&repo_configs().join(DEFAULTS_FILE)).unwrap();
        merge(
            &mut t,
            table("[dataset]\nname = \"x\"\ninput = \"x\"\n[gates]\nmin_registered_ratio = 1.5\n"),
        );
        assert!(matches!(Config::from_table(t), Err(Error::Config(_))));
    }
}
