//! Run configuration.
//!
//! Layers, each deep-merged over the previous one (ADR-0017): the defaults
//! built into the binary, the user's `config.toml`, `defaults.toml` beside a
//! dataset config, the dataset config itself, then command-line overrides.
//! Tables merge recursively; any other value replaces the one below it.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use toml::{Table, Value};

use crate::error::{Error, Result};

pub const DEFAULTS_FILE: &str = "defaults.toml";
/// The user's config layer, in [`crate::dirs::config_dir`].
pub const USER_CONFIG_FILE: &str = "config.toml";
/// `configs/defaults.toml`, built in so an installed binary needs no checkout.
pub const EMBEDDED_DEFAULTS: &str = include_str!("../../../configs/defaults.toml");

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
    pub export: Export,
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
    /// Maximum Hamming distance between 8x8 perceptual hashes for two
    /// frames to be treated as near-duplicates (0 = exact duplicates only).
    pub dedup_threshold: u32,
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
    /// Length of each processing-step clip (frames, masks, sparse, ...).
    pub step_seconds: f64,
    pub fps: u32,
    pub width: u32,
    pub height: u32,
    /// Size limit of the share copy, in MiB (for chat and mail attachments).
    pub share_max_mib: f64,
}

/// What the export stage delivers. Kept apart from `[video]`, whose whole
/// table is the video stage's fingerprint input (ADR-0017).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Export {
    /// Run the video stage and deliver its videos; models only when false.
    pub videos: bool,
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
    /// Load a dataset config over the built-in defaults and `defaults.toml`
    /// beside it. Reads nothing from the user's directories.
    pub fn load(path: &Path) -> Result<Self> {
        Self::layered(None, &[path], Table::new())
    }

    /// Every layer: built-in defaults, `user_dir/config.toml`, then for each
    /// file `defaults.toml` beside it and the file, then `overrides`.
    pub fn layered(user_dir: Option<&Path>, files: &[&Path], overrides: Table) -> Result<Self> {
        let mut table = embedded_defaults()?;
        for layer in layer_files(user_dir, files) {
            merge(&mut table, read_table(&layer)?);
        }
        merge(&mut table, overrides);
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
        if self.frames.dedup_threshold > 64 {
            return invalid("frames.dedup_threshold must be at most 64");
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
        if !(self.video.step_seconds.is_finite() && self.video.step_seconds > 0.0) {
            return invalid("video.step_seconds must be a positive finite number");
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

pub fn embedded_defaults() -> Result<Table> {
    EMBEDDED_DEFAULTS
        .parse::<Table>()
        .map_err(|source| Error::Parse {
            path: PathBuf::from("<built-in defaults.toml>"),
            source: Box::new(source),
        })
}

/// The config files [`Config::layered`] reads, lowest first; missing optional
/// files (the user layer, a beside `defaults.toml`) are skipped.
pub fn layer_files(user_dir: Option<&Path>, files: &[&Path]) -> Vec<PathBuf> {
    let mut layers: Vec<PathBuf> = user_dir
        .map(|d| d.join(USER_CONFIG_FILE))
        .filter(|p| p.is_file())
        .into_iter()
        .collect();
    for file in files {
        let beside = file
            .parent()
            .map(|dir| dir.join(DEFAULTS_FILE))
            .filter(|p| p.is_file() && !same_file(p, file));
        if let Some(beside) = beside.filter(|b| !layers.iter().any(|l| same_file(l, b))) {
            layers.push(beside);
        }
        layers.push(file.to_path_buf());
    }
    layers
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
    fn embedded_defaults_are_the_repo_defaults() {
        assert_eq!(
            embedded_defaults().unwrap(),
            read_table(&repo_configs().join(DEFAULTS_FILE)).unwrap()
        );
    }

    #[test]
    fn a_lone_dataset_file_loads_over_the_built_in_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("x.toml");
        std::fs::write(&file, "[dataset]\nname = \"x\"\ninput = \"x.mp4\"\n").unwrap();
        let cfg = Config::load(&file).unwrap();
        assert_eq!(cfg.dataset.name, "x");
        assert_eq!(cfg.sfm.max_features, 16384);
    }

    #[test]
    fn layers_apply_lowest_first() {
        let user = tempfile::tempdir().unwrap();
        std::fs::write(
            user.path().join(USER_CONFIG_FILE),
            "[splat]\nsteps = 1\n[frames]\nfps = 1.0\n",
        )
        .unwrap();
        let data = tempfile::tempdir().unwrap();
        std::fs::write(data.path().join(DEFAULTS_FILE), "[splat]\nsteps = 2\n").unwrap();
        let file = data.path().join("x.toml");
        std::fs::write(
            &file,
            "[dataset]\nname = \"x\"\ninput = \"x.mp4\"\n[sfm]\nmax_features = 3\n",
        )
        .unwrap();
        let cfg = Config::layered(
            Some(user.path()),
            &[&file],
            table("[sfm]\nmax_features = 4\n"),
        )
        .unwrap();
        assert_eq!(cfg.frames.fps, 1.0); // user over built-in
        assert_eq!(cfg.splat.steps, 2); // beside defaults over user
        assert_eq!(cfg.sfm.max_features, 4); // overrides over the file
        assert_eq!(cfg.masks.dilate_px, 8); // built-in
    }

    #[test]
    fn layer_files_skip_missing_and_repeated_defaults() {
        let dir = repo_configs();
        let defaults = dir.join(DEFAULTS_FILE);
        let dataset = dir.join("reposalmi-laajasalo.toml");
        assert_eq!(
            layer_files(Some(Path::new("/nonexistent")), &[&dataset]),
            [defaults.clone(), dataset.clone()]
        );
        assert_eq!(
            layer_files(None, &[&defaults]),
            std::slice::from_ref(&defaults)
        );
        assert_eq!(
            layer_files(None, &[&dataset, &dataset]),
            [defaults, dataset.clone(), dataset]
        );
    }

    #[test]
    fn videos_are_off_by_default_and_on_for_the_repo_datasets() {
        let lone = Config::layered(
            None,
            &[],
            table("[dataset]\nname = \"x\"\ninput = \"x.mp4\"\n"),
        )
        .unwrap();
        assert!(!lone.export.videos);
        let repo = Config::load(&repo_configs().join("reposalmi-laajasalo.toml")).unwrap();
        assert!(repo.export.videos);
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
            "[video]\nstep_seconds = 0.0\n",
            "[video]\nstep_seconds = nan\n",
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
