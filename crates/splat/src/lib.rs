//! `splat` stage: Gaussian splat training with Brush (ADR-0011).
//!
//! Undistorts the frames with COLMAP (Brush reads only focal length and
//! principal point), warps the masks into the undistorted cameras under the
//! names Brush matches (`masks/00001.png` for `images/00001.jpg`), and trains
//! with `brush_app`. Output: `dataset/` (Brush's input), `splat.ply` and
//! `logs/<step>.log`.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use shoreline_core::error::io_error;
use shoreline_core::stage::{Inputs, StageRun};
use shoreline_core::tools::Invocation;
use shoreline_core::{Config, Error, Result, RunContext, Stage};
use shoreline_sfm::{Model, ply, undistort};

pub const NAME: &str = "splat";
const FRAMES: &str = "frames";
const MASKS: &str = "masks";
const SFM: &str = "sfm";
const COLMAP: &str = "colmap";
const BRUSH: &str = "brush";

/// Bump when the output changes for the same config, inputs and tools.
const REVISION: u32 = 1;

/// Metric names.
pub const GAUSSIANS: &str = "gaussians";
pub const TRAIN_IMAGES: &str = "train_images";

pub struct Splat;

/// `brush_app` arguments: train every frame for `steps` and export once, at the end.
pub fn brush_args(dataset: &Path, out: &Path, steps: u32) -> Vec<OsString> {
    let steps = steps.to_string();
    let mut args: Vec<OsString> = vec![dataset.into()];
    args.extend(["--total-steps", &steps, "--export-every", &steps].map(OsString::from));
    args.extend([OsString::from("--export-path"), out.into()]);
    args.extend(["--export-name", "splat_{iter}.ply"].map(OsString::from));
    args
}

/// Brush's input: the undistorter's `images/`, the model in `sparse/0/` and
/// `masks/`. The undistorter's `stereo/` and scripts are removed.
fn tidy_dataset(dataset: &Path) -> Result<PathBuf> {
    let zero = undistort::sparse_to_sparse0(dataset)?;
    let stereo = dataset.join("stereo");
    std::fs::remove_dir_all(&stereo).map_err(io_error(&stereo))?;
    for entry in std::fs::read_dir(dataset).map_err(io_error(dataset))? {
        let path = entry.map_err(io_error(dataset))?.path();
        if path.extension().is_some_and(|e| e == "sh") {
            std::fs::remove_file(&path).map_err(io_error(&path))?;
        }
    }
    Ok(zero)
}

impl Stage for Splat {
    fn name(&self) -> &'static str {
        NAME
    }

    fn deps(&self) -> &[&'static str] {
        &[FRAMES, MASKS, SFM]
    }

    fn config(&self, config: &Config) -> serde_json::Value {
        serde_json::to_value(&config.splat).expect("splat config serialises to JSON")
    }

    fn version(&self, ctx: &RunContext) -> Result<String> {
        if ctx.config.splat.backend != BRUSH {
            return Err(stage_error(format!(
                "splat.backend {:?} is not implemented (implemented: brush)",
                ctx.config.splat.backend
            )));
        }
        let colmap = ctx.tools.get(COLMAP)?.version(&["help"])?;
        let brush = ctx.tools.get(BRUSH)?.version(&["--version"])?;
        Ok(format!("{NAME}/{REVISION} {colmap} {brush}"))
    }

    fn steps(&self, _: &Config) -> Vec<String> {
        vec![undistort::LOG.to_owned(), BRUSH.to_owned()]
    }

    fn run(
        &self,
        ctx: &RunContext,
        inputs: &Inputs,
        out_dir: &Path,
        _on_event: &mut dyn FnMut(shoreline_core::pipeline::Event<'_>),
    ) -> Result<StageRun> {
        let images = inputs[FRAMES].dir.join("images");
        let masks = inputs[MASKS].dir.join("masks");
        let sparse = inputs[SFM].dir.join("sparse/0");
        let dataset = out_dir.join("dataset");
        let logs = out_dir.join("logs");
        let mount = |args| {
            Invocation::new(args)
                .mount(&images)
                .mount(&masks)
                .mount(&inputs[SFM].dir)
                .mount(out_dir)
        };

        let colmap = ctx.tools.get(COLMAP)?;
        let undistort_args = undistort::undistorter_args(&images, &sparse, &dataset);
        let mut tools = vec![colmap.run(
            &mount(undistort_args),
            &logs.join(format!("{}.log", undistort::LOG)),
        )?];
        let zero = tidy_dataset(&dataset)?;
        let distorted = Model::read_binary(&sparse)?;
        let undistorted = Model::read_binary(&zero)?;
        let n = undistort::undistort_masks(
            &distorted,
            &undistorted,
            &masks,
            &dataset.join("masks"),
            |name| format!("{}.png", undistort::stem(name)),
        )?;

        let steps = ctx.config.splat.steps;
        let brush = ctx.tools.get(BRUSH)?;
        let args = brush_args(&dataset, out_dir, steps);
        tools.push(brush.run(&mount(args), &logs.join(format!("{BRUSH}.log")))?);
        let exported = out_dir.join(format!("splat_{steps}.ply"));
        let splat = out_dir.join("splat.ply");
        std::fs::rename(&exported, &splat).map_err(|_| {
            stage_error(format!(
                "Brush wrote no {} (see logs/brush.log)",
                exported.display()
            ))
        })?;

        let gaussians = ply::element_counts(&splat)?
            .get("vertex")
            .copied()
            .unwrap_or(0);
        let metrics = [(GAUSSIANS, gaussians as f64), (TRAIN_IMAGES, n as f64)]
            .into_iter()
            .map(|(k, v)| (k.to_owned(), v))
            .collect();
        Ok(StageRun { tools, metrics })
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

    fn strings(args: &[OsString]) -> Vec<String> {
        args.iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn steps_are_the_log_names_in_run_order() {
        let config = Config::layered(
            None,
            &[],
            "[dataset]\nname='x'\ninput='x.mp4'".parse().unwrap(),
        )
        .unwrap();
        assert_eq!(Splat.steps(&config), ["image_undistorter", "brush"]);
    }

    #[test]
    fn brush_trains_every_frame_and_exports_once() {
        let args = brush_args(Path::new("/o/dataset"), Path::new("/o"), 30000);
        assert_eq!(
            strings(&args),
            [
                "/o/dataset",
                "--total-steps",
                "30000",
                "--export-every",
                "30000",
                "--export-path",
                "/o",
                "--export-name",
                "splat_{iter}.ply"
            ]
        );
    }

    #[test]
    fn the_dataset_keeps_images_masks_and_sparse_0_only() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path();
        for sub in ["images", "sparse", "stereo/depth_maps"] {
            std::fs::create_dir_all(d.join(sub)).unwrap();
        }
        for f in [
            "sparse/cameras.bin",
            "sparse/images.bin",
            "sparse/points3D.bin",
            "run-colmap-geometric.sh",
        ] {
            std::fs::write(d.join(f), b"").unwrap();
        }
        let zero = tidy_dataset(d).unwrap();
        assert!(zero.join("images.bin").exists());
        assert!(!d.join("stereo").exists() && !d.join("run-colmap-geometric.sh").exists());
        assert!(d.join("images").is_dir());
    }

    #[test]
    fn version_needs_the_brush_backend() {
        let image = "x/y@sha256:c1de51615dff33066cd305071226b6343c1168324ef3bd622f0825e55feb5efe";
        let table = format!(
            "[colmap]\nruntime = \"apptainer\"\nprogram = \"colmap\"\nimage = \"{image}\"\n\
             [brush]\nruntime = \"apptainer\"\nprogram = \"brush_app\"\nimage = \"{image}\"\n"
        );
        let tools = shoreline_core::Tools::from_table(table.parse().unwrap()).unwrap();
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../configs");
        let mut config = Config::load(&dir.join("reposalmi-laajasalo.toml")).unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let ctx = RunContext::new(config.clone(), tools.clone(), tmp.path()).unwrap();
        assert_eq!(
            Splat.version(&ctx).unwrap(),
            format!("splat/1 {image} {image}")
        );
        config.splat.backend = "splatfacto".into();
        let ctx = RunContext::new(config, tools, tmp.path()).unwrap();
        assert!(Splat.version(&ctx).is_err());
    }
}
