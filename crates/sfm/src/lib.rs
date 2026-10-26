//! `sfm` stage: COLMAP structure-from-motion over the frames, with the masks
//! applied at feature extraction (kickoff § sfm, ADR-0002, ADR-0010).
//!
//! Runs `colmap feature_extractor`, the configured matcher and `mapper` with
//! the spike's settings. Output: `database.db`, `sparse/0/` (the largest model
//! when the mapper splits the capture) and `logs/<step>.log`. The metrics are
//! computed from the model ([`metrics`]).

pub mod metrics;
pub mod model;
pub mod ply;
pub mod undistort;

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use shoreline_core::config::{Matcher, Sfm as SfmConfig};
use shoreline_core::error::io_error;
use shoreline_core::stage::{Inputs, Metrics, StageRun};
use shoreline_core::tools::Invocation;
use shoreline_core::{Config, Error, Result, RunContext, Stage};

pub use model::Model;

pub const NAME: &str = "sfm";
const FRAMES: &str = "frames";
const MASKS: &str = "masks";
const COLMAP: &str = "colmap";

/// Bump when the output changes for the same config, inputs and tool.
const REVISION: u32 = 1;

/// Number of models the mapper wrote, recorded with the model metrics.
pub const MODELS: &str = "models";

pub struct Sfm;

/// Where one run reads and writes.
#[derive(Debug, Clone)]
pub struct Paths {
    pub images: PathBuf,
    pub masks: PathBuf,
    pub out: PathBuf,
}

impl Paths {
    fn database(&self) -> PathBuf {
        self.out.join("database.db")
    }
    fn sparse(&self) -> PathBuf {
        self.out.join("sparse")
    }
}

/// COLMAP calls in order, as `(log name, arguments)`.
pub fn steps(sfm: &SfmConfig, paths: &Paths, gpu: bool) -> Vec<(&'static str, Vec<OsString>)> {
    let flag = |b: bool| if b { "1" } else { "0" };
    let db = paths.database();
    let matcher = match sfm.matcher {
        Matcher::Sequential => "sequential_matcher",
        Matcher::Exhaustive => "exhaustive_matcher",
    };
    let args = |words: &[&dyn AsRef<std::ffi::OsStr>]| -> Vec<OsString> {
        words.iter().map(|w| w.as_ref().to_owned()).collect()
    };
    vec![
        (
            "feature_extractor",
            args(&[
                &"feature_extractor",
                &"--database_path",
                &db,
                &"--image_path",
                &paths.images,
                &"--ImageReader.mask_path",
                &paths.masks,
                &"--ImageReader.single_camera",
                &flag(sfm.single_camera),
                &"--SiftExtraction.max_num_features",
                &sfm.max_features.to_string(),
                &"--FeatureExtraction.use_gpu",
                &flag(gpu),
            ]),
        ),
        (
            matcher,
            args(&[
                &matcher,
                &"--database_path",
                &db,
                &"--FeatureMatching.use_gpu",
                &flag(gpu),
            ]),
        ),
        (
            "mapper",
            args(&[
                &"mapper",
                &"--database_path",
                &db,
                &"--image_path",
                &paths.images,
                &"--output_path",
                &paths.sparse(),
            ]),
        ),
    ]
}

/// Read every `sparse/<n>` model, make the one with the most registered images
/// `sparse/0`, and return it with the number of models.
pub fn select_model(sparse: &Path) -> Result<(Model, usize)> {
    let mut models = Vec::new();
    for entry in std::fs::read_dir(sparse).map_err(io_error(sparse))? {
        let entry = entry.map_err(io_error(sparse))?;
        let name = entry.file_name();
        if let Some(n) = name.to_str().and_then(|n| n.parse::<u32>().ok()) {
            models.push((n, Model::read_binary(&entry.path())?));
        }
    }
    // Most images first; the lower index wins a tie.
    models.sort_by(|(a, ma), (b, mb)| mb.images.len().cmp(&ma.images.len()).then(a.cmp(b)));
    let count = models.len();
    let Some((best, model)) = models.into_iter().next() else {
        return Err(stage_error(format!(
            "mapper wrote no model under {}",
            sparse.display()
        )));
    };
    if best != 0 {
        let (zero, chosen, aside) = (
            sparse.join("0"),
            sparse.join(best.to_string()),
            sparse.join("swap"),
        );
        std::fs::rename(&zero, &aside).map_err(io_error(&zero))?;
        std::fs::rename(&chosen, &zero).map_err(io_error(&chosen))?;
        std::fs::rename(&aside, &chosen).map_err(io_error(&aside))?;
    }
    Ok((model, count))
}

/// The `.jpg` frames in `images`, which the frames stage writes.
fn count_frames(images: &Path) -> Result<usize> {
    let mut n = 0;
    for entry in std::fs::read_dir(images).map_err(io_error(images))? {
        let name = entry.map_err(io_error(images))?.file_name();
        n += usize::from(name.to_str().is_some_and(|n| n.ends_with(".jpg")));
    }
    match n {
        0 => Err(stage_error(format!("no frames in {}", images.display()))),
        n => Ok(n),
    }
}

impl Stage for Sfm {
    fn name(&self) -> &'static str {
        NAME
    }

    fn deps(&self) -> &[&'static str] {
        &[FRAMES, MASKS]
    }

    fn config(&self, config: &Config) -> serde_json::Value {
        serde_json::to_value(&config.sfm).expect("sfm config serialises to JSON")
    }

    fn version(&self, ctx: &RunContext) -> Result<String> {
        if ctx.config.sfm.backend != COLMAP {
            return Err(stage_error(format!(
                "sfm.backend {:?} is not implemented (implemented: colmap)",
                ctx.config.sfm.backend
            )));
        }
        let colmap = ctx.tools.get(COLMAP)?;
        Ok(format!("{NAME}/{REVISION} {}", colmap.version(&["help"])?))
    }

    fn run(&self, ctx: &RunContext, inputs: &Inputs, out_dir: &Path) -> Result<StageRun> {
        let paths = Paths {
            images: inputs[FRAMES].dir.join("images"),
            masks: inputs[MASKS].dir.join("masks"),
            out: out_dir.to_owned(),
        };
        let input_images = count_frames(&paths.images)?;
        let sparse = paths.sparse();
        std::fs::create_dir_all(&sparse).map_err(io_error(&sparse))?;

        let colmap = ctx.tools.get(COLMAP)?;
        let mut tools = Vec::new();
        for (log, args) in steps(&ctx.config.sfm, &paths, colmap.spec.gpu) {
            let inv = Invocation::new(args)
                .mount(&paths.images)
                .mount(&paths.masks)
                .mount(out_dir);
            tools.push(colmap.run(&inv, &out_dir.join(format!("logs/{log}.log")))?);
        }

        let (model, models) = select_model(&sparse)?;
        let mut metrics = metrics::metrics(&model, input_images);
        metrics.insert(MODELS.to_owned(), models as f64);
        Ok(StageRun { tools, metrics })
    }

    fn gate(&self, config: &Config, metrics: &Metrics) -> Result<Vec<String>> {
        metrics::gate(config, metrics)
    }
}

fn stage_error(message: impl Into<String>) -> Error {
    Error::Stage {
        stage: NAME,
        message: message.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::tests::{sample, write_binary};

    fn repo_config(name: &str) -> Config {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../configs");
        Config::load(&dir.join(format!("{name}.toml"))).unwrap()
    }

    fn paths() -> Paths {
        Paths {
            images: "/w/frames/f/images".into(),
            masks: "/w/masks/m/masks".into(),
            out: "/w/sfm/s".into(),
        }
    }

    fn strings(args: &[OsString]) -> Vec<String> {
        args.iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn steps_apply_masks_at_extraction_with_spike_settings() {
        let config = repo_config("reposalmi-laajasalo");
        let steps = steps(&config.sfm, &paths(), true);
        let names: Vec<_> = steps.iter().map(|(n, _)| *n).collect();
        assert_eq!(names, ["feature_extractor", "sequential_matcher", "mapper"]);
        assert_eq!(
            strings(&steps[0].1),
            [
                "feature_extractor",
                "--database_path",
                "/w/sfm/s/database.db",
                "--image_path",
                "/w/frames/f/images",
                "--ImageReader.mask_path",
                "/w/masks/m/masks",
                "--ImageReader.single_camera",
                "1",
                "--SiftExtraction.max_num_features",
                "16384",
                "--FeatureExtraction.use_gpu",
                "1",
            ]
        );
        assert_eq!(
            strings(&steps[1].1),
            [
                "sequential_matcher",
                "--database_path",
                "/w/sfm/s/database.db",
                "--FeatureMatching.use_gpu",
                "1"
            ]
        );
        assert_eq!(
            strings(&steps[2].1),
            [
                "mapper",
                "--database_path",
                "/w/sfm/s/database.db",
                "--image_path",
                "/w/frames/f/images",
                "--output_path",
                "/w/sfm/s/sparse",
            ]
        );
    }

    #[test]
    fn vartiosaari_matches_exhaustively_and_cpu_tools_disable_the_gpu() {
        let config = repo_config("reposalmi-vartiosaari");
        let steps = steps(&config.sfm, &paths(), false);
        assert_eq!(steps[1].0, "exhaustive_matcher");
        assert_eq!(strings(&steps[1].1).last().unwrap(), "0");
        assert_eq!(strings(&steps[0].1).last().unwrap(), "0");
    }

    #[test]
    fn the_largest_model_becomes_sparse_0() {
        let dir = tempfile::tempdir().unwrap();
        let sparse = dir.path();
        let mut small = sample();
        small.images.remove(&2);
        small.points3d.retain(|_, p| {
            p.track.retain(|(i, _)| *i == 1);
            true
        });
        small.images.get_mut(&1).unwrap().name = "small".into();
        write_binary(&small, &sparse.join("0"));
        write_binary(&sample(), &sparse.join("1"));
        std::fs::create_dir(sparse.join("notes")).unwrap();

        let (model, count) = select_model(sparse).unwrap();
        assert_eq!((model.images.len(), count), (2, 2));
        assert_eq!(Model::read_binary(&sparse.join("0")).unwrap(), sample());
        assert_eq!(Model::read_binary(&sparse.join("1")).unwrap(), small);
        assert!(!sparse.join("swap").exists());
    }

    #[test]
    fn no_model_is_a_stage_error() {
        let dir = tempfile::tempdir().unwrap();
        let err = select_model(dir.path()).unwrap_err();
        assert!(
            matches!(&err, Error::Stage { message, .. } if message.contains("no model")),
            "{err}"
        );
    }

    #[test]
    fn frames_are_the_jpgs_in_images() {
        let dir = tempfile::tempdir().unwrap();
        for name in ["00001.jpg", "00002.jpg", "notes.txt"] {
            std::fs::write(dir.path().join(name), b"").unwrap();
        }
        assert_eq!(count_frames(dir.path()).unwrap(), 2);
        let empty = tempfile::tempdir().unwrap();
        assert!(count_frames(empty.path()).is_err());
    }

    #[test]
    fn version_needs_the_colmap_backend_and_names_the_image() {
        let image =
            "colmap/colmap@sha256:c1de51615dff33066cd305071226b6343c1168324ef3bd622f0825e55feb5efe";
        let tools = shoreline_core::Tools::from_table(
            format!(
                "[colmap]\nruntime = \"apptainer\"\nprogram = \"colmap\"\nimage = \"{image}\"\n"
            )
            .parse()
            .unwrap(),
        )
        .unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let mut config = repo_config("reposalmi-laajasalo");
        let ctx = RunContext::new(config.clone(), tools.clone(), tmp.path()).unwrap();
        assert_eq!(Sfm.version(&ctx).unwrap(), format!("sfm/1 {image}"));

        config.sfm.backend = "glomap".into();
        let ctx = RunContext::new(config, tools, tmp.path()).unwrap();
        assert!(
            matches!(Sfm.version(&ctx), Err(Error::Stage { message, .. }) if message.contains("glomap"))
        );
    }
}
