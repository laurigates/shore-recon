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
    pub video: Video,
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
    pub backend: MasksBackend,
    /// Model file for the `onnx` backend; the container image carries its own.
    pub model: Option<PathBuf>,
    pub classes: Vec<String>,
    pub dilate_px: u32,
    pub temporal: bool,
}

/// Segmentation backend: the pinned container image (ADR-0006), or ONNX
/// through `ort` (#11, not implemented yet).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MasksBackend {
    Container,
    Onnx,
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
    /// `"fit"`, or a fixed height along the camera-derived up axis.
    pub water_plane: WaterPlane,
    /// Rows above an ignored mask region that count as waterline (ADR-0012).
    pub waterline_band_px: u32,
    pub min_component: f64,
    pub target_faces: u32,
    /// Longest edge of the texture embedded in `model.glb`.
    pub texture_max_size: u32,
    /// Largest fraction of a Gaussian's views that may see it against masked water.
    pub floater_max_water: f64,
}

/// Camera path of the showcase video (`render_video --path`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CameraPath {
    Arc,
    Drone,
}

/// One camera path rendered for every scene, and its loop length.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct View {
    pub path: CameraPath,
    pub seconds: f64,
}

/// The showcase videos of the splat and the mesh (ADR-0008), one per view.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Video {
    pub views: Vec<View>,
    pub fps: u32,
    pub width: u32,
    pub height: u32,
    /// Size limit of the share copy, in MiB (Telegram's bot upload limit is 50 MB).
    pub share_max_mib: f64,
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
        if !is_path_component(&self.dataset.name) {
            return invalid(
                "dataset.name must start with a letter or digit and contain only \
                 letters, digits, '.', '_' and '-' (it names a directory under work/)",
            );
        }
        match (self.masks.backend, &self.masks.model) {
            (MasksBackend::Onnx, _) => {
                return invalid(
                    "masks.backend = \"onnx\" is not implemented yet (#11); use \"container\"",
                );
            }
            (MasksBackend::Container, Some(_)) => {
                return invalid(
                    "masks.model is only used by the onnx backend; the container image carries its own",
                );
            }
            (MasksBackend::Container, None) => {}
        }
        if self.masks.temporal {
            return invalid("masks.temporal is not implemented yet");
        }
        if self.masks.classes.is_empty() {
            return invalid("masks.classes must not be empty");
        }
        let numbers = [
            ("frames.fps", self.frames.fps),
            ("frames.blur_threshold", self.frames.blur_threshold),
            ("post.min_component", self.post.min_component),
            ("post.floater_max_water", self.post.floater_max_water),
            ("video.share_max_mib", self.video.share_max_mib),
            (
                "gates.min_registered_ratio",
                self.gates.min_registered_ratio,
            ),
            ("gates.max_reproj_error_px", self.gates.max_reproj_error_px),
            ("gates.min_track_length", self.gates.min_track_length),
            ("gates.max_masked_fraction", self.gates.max_masked_fraction),
        ];
        let height = match self.post.water_plane {
            WaterPlane::Height(h) => Some(("post.water_plane", h)),
            WaterPlane::Fit(_) => None,
        };
        // Non-finite floats all serialise to JSON null, so they would share a fingerprint.
        if let Some((key, _)) = numbers
            .into_iter()
            .chain(height)
            .find(|(_, v)| !v.is_finite())
        {
            return Err(Error::Config(format!("{key} must be a finite number")));
        }
        if self.frames.fps <= 0.0 {
            return invalid("frames.fps must be positive");
        }
        if self.video.views.is_empty() {
            return invalid("video.views must list at least one view");
        }
        for (i, v) in self.video.views.iter().enumerate() {
            if !(v.seconds.is_finite() && v.seconds > 0.0) {
                return invalid("video.views[].seconds must be a positive finite number");
            }
            if self.video.views[..i].iter().any(|w| w.path == v.path) {
                return invalid("video.views lists the same path twice");
            }
        }
        if self.video.share_max_mib <= 0.0 {
            return invalid("video.share_max_mib must be positive");
        }
        if self.video.fps == 0 || self.video.width == 0 || self.video.height == 0 {
            return invalid("video.fps, video.width and video.height must be positive");
        }
        if !(0.0..=1.0).contains(&self.post.min_component) {
            return invalid("post.min_component must be within [0, 1]");
        }
        if !(0.0..=1.0).contains(&self.post.floater_max_water) {
            return invalid("post.floater_max_water must be within [0, 1]");
        }
        if self.post.waterline_band_px == 0
            || self.post.target_faces == 0
            || self.post.texture_max_size == 0
        {
            return invalid(
                "post.waterline_band_px, post.target_faces and post.texture_max_size must be positive",
            );
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

/// A single, plain path component: no separators, not `.` or `..`.
fn is_path_component(name: &str) -> bool {
    name.starts_with(|c: char| c.is_ascii_alphanumeric())
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
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

pub(crate) fn read_table(path: &Path) -> Result<Table> {
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
            toml::from_str("water_plane = \"fit\"\nwaterline_band_px = 24\nmin_component = 0.01\ntarget_faces = 1\ntexture_max_size = 8\nfloater_max_water = 0.5\n")
                .unwrap();
        assert_eq!(fit.water_plane, WaterPlane::Fit(FitTag::Fit));
        let fixed: Post =
            toml::from_str("water_plane = -0.5\nwaterline_band_px = 24\nmin_component = 0.01\ntarget_faces = 1\ntexture_max_size = 8\nfloater_max_water = 0.5\n").unwrap();
        assert_eq!(fixed.water_plane, WaterPlane::Height(-0.5));
        assert!(
            toml::from_str::<Post>(
                "water_plane = \"auto\"\nwaterline_band_px = 24\nmin_component = 0.01\ntarget_faces = 1\ntexture_max_size = 8\nfloater_max_water = 0.5\n"
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

    fn with_overrides(overrides: &str) -> Result<Config> {
        let mut t = read_table(&repo_configs().join(DEFAULTS_FILE)).unwrap();
        merge(&mut t, table("[dataset]\nname = \"x\"\ninput = \"x\"\n"));
        merge(&mut t, table(overrides));
        Config::from_table(t)
    }

    #[test]
    fn video_settings_are_validated() {
        let c = with_overrides("").unwrap();
        assert_eq!(c.video.fps, 30);
        assert_eq!(
            c.video.views,
            [
                View {
                    path: CameraPath::Arc,
                    seconds: 16.0
                },
                View {
                    path: CameraPath::Drone,
                    seconds: 20.0
                },
            ]
        );
        let one =
            with_overrides("[video]\nviews = [{ path = \"drone\", seconds = 8.0 }]\n").unwrap();
        assert_eq!(
            one.video.views,
            [View {
                path: CameraPath::Drone,
                seconds: 8.0
            }]
        );
        for bad in [
            "[video]\nviews = []\n",
            "[video]\nviews = [{ path = \"orbit\", seconds = 1.0 }]\n",
            "[video]\nviews = [{ path = \"arc\", seconds = 0.0 }]\n",
            "[video]\nviews = [{ path = \"arc\", seconds = nan }]\n",
            "[video]\nviews = [{ path = \"arc\", seconds = 1.0 }, { path = \"arc\", seconds = 2.0 }]\n",
            "[video]\npath = \"arc\"\n",
            "[video]\nshare_max_mib = -1.0\n",
            "[video]\nfps = 0\n",
            "[video]\nwidth = 0\n",
        ] {
            assert!(with_overrides(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn masks_backend_options() {
        let defaults = with_overrides("").unwrap();
        assert_eq!(defaults.masks.backend, MasksBackend::Container);
        assert_eq!(defaults.masks.model, None);
        for (overrides, needle) in [
            ("[masks]\nbackend = \"onnx\"\n", "#11"),
            ("[masks]\nmodel = \"models/x.onnx\"\n", "masks.model"),
            ("[masks]\ntemporal = true\n", "masks.temporal"),
            ("[masks]\nclasses = []\n", "masks.classes"),
            ("[masks]\nbackend = \"sam2\"\n", "sam2"),
        ] {
            let err = with_overrides(overrides).unwrap_err().to_string();
            assert!(err.contains(needle), "{overrides:?}: {err}");
        }
    }

    #[test]
    fn dataset_name_must_be_one_plain_path_component() {
        for bad in [
            "", ".", "..", "../data", "/abs", "a/b", "-x", ".hidden", "a b", "å",
        ] {
            let err = with_overrides(&format!("[dataset]\nname = \"{bad}\"\n")).unwrap_err();
            assert!(err.to_string().contains("dataset.name"), "{bad:?}: {err}");
        }
        for good in ["reposalmi-laajasalo", "a.b_c-1", "X1"] {
            with_overrides(&format!("[dataset]\nname = \"{good}\"\n")).unwrap();
        }
    }

    #[test]
    fn non_finite_numbers_are_rejected() {
        for (section, key, value) in [
            ("frames", "fps", "nan"),
            ("frames", "blur_threshold", "inf"),
            ("gates", "max_reproj_error_px", "nan"),
            ("gates", "min_track_length", "-inf"),
            ("post", "water_plane", "-inf"),
        ] {
            let err = with_overrides(&format!("[{section}]\n{key} = {value}\n")).unwrap_err();
            assert!(
                err.to_string().contains(&format!("{section}.{key}")),
                "{key}: {err}"
            );
        }
        with_overrides("[post]\nwater_plane = -0.5\n").unwrap();
    }
}
