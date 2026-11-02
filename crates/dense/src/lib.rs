//! `dense` stage: OpenMVS densification, meshing and texturing (ADR-0011).
//!
//! Undistorts the frames with COLMAP into `undistorted/` (OpenMVS reads
//! COLMAP's undistorted workspace), warps the masks into the undistorted
//! cameras as `masks/00001.mask.png`, then runs `InterfaceCOLMAP`,
//! `DensifyPointCloud` (masks native, black = ignore), `ReconstructMesh` and
//! `TextureMesh`. Output: `scene_dense.ply` (points), `mesh.ply`,
//! `mesh_textured.glb` and `logs/<step>.log`.
//!
//! The `openmvs` tool's program is a dispatcher (`env` in the image): each step
//! passes the OpenMVS executable as its first argument.

use std::ffi::OsString;
use std::path::Path;

use shoreline_core::stage::{Inputs, StageRun};
use shoreline_core::tools::Invocation;
use shoreline_core::{Config, Error, Result, RunContext, Stage};
use shoreline_sfm::{Model, ply, undistort};

pub const NAME: &str = "dense";
const FRAMES: &str = "frames";
const MASKS: &str = "masks";
const SFM: &str = "sfm";
const COLMAP: &str = "colmap";
const OPENMVS: &str = "openmvs";
const TEXTURE_MESH: &str = "TextureMesh";
/// Smallest ratio passed to `--decimate`: the six decimals it is formatted with.
const MIN_DECIMATE: f64 = 1e-6;

/// Bump when the output changes for the same config, inputs and tools.
const REVISION: u32 = 1;

/// Metric names.
pub const DENSE_POINTS: &str = "dense_points";
pub const MESH_FACES: &str = "mesh_faces";
pub const MESH_VERTICES: &str = "mesh_vertices";

pub struct Dense;

/// OpenMVS steps in order, as `(log name, arguments)`. All paths are under `out`.
pub fn openmvs_steps(out: &Path) -> Vec<(&'static str, Vec<OsString>)> {
    let p = |name: &str| out.join(name).into_os_string();
    let args = |words: Vec<OsString>| words;
    let w = |v: &str| OsString::from(v);
    vec![
        (
            "InterfaceCOLMAP",
            args(vec![
                w("InterfaceCOLMAP"),
                w("-w"),
                p(""),
                w("-i"),
                p("undistorted"),
                w("--image-folder"),
                p("undistorted/images"),
                w("-o"),
                p("scene.mvs"),
            ]),
        ),
        (
            "DensifyPointCloud",
            args(vec![
                w("DensifyPointCloud"),
                w("-w"),
                p(""),
                w("-i"),
                p("scene.mvs"),
                w("-o"),
                p("scene_dense.mvs"),
                w("--mask-path"),
                p("masks"),
                w("--ignore-mask-label"),
                w("0"),
                w("--remove-dmaps"),
                w("1"),
            ]),
        ),
        (
            "ReconstructMesh",
            args(vec![
                w("ReconstructMesh"),
                w("-w"),
                p(""),
                w("-i"),
                p("scene_dense.mvs"),
                w("-o"),
                p("mesh.ply"),
            ]),
        ),
        (TEXTURE_MESH, texture_mesh_args(out, None)),
    ]
}

/// `TextureMesh` arguments, with `--decimate <ratio>` when `ratio` is given.
pub fn texture_mesh_args(out: &Path, ratio: Option<f64>) -> Vec<OsString> {
    let p = |name: &str| out.join(name).into_os_string();
    let w = |v: &str| OsString::from(v);
    let mut args = vec![
        w(TEXTURE_MESH),
        w("-w"),
        p(""),
        w("-i"),
        p("scene_dense.mvs"),
        w("-m"),
        p("mesh.ply"),
        w("--export-type"),
        w("glb"),
        w("-o"),
        p("mesh_textured.glb"),
    ];
    if let Some(ratio) = ratio {
        args.push(w("--decimate"));
        args.push(OsString::from(format!("{ratio:.6}")));
    }
    args
}

/// The `TextureMesh --decimate` ratio that brings `faces` down to
/// `target_faces * headroom`, or `None` when the mesh is already at or under
/// that (or empty). Clamped to 1e-6 so it never formats as `0.000000`, which
/// OpenMVS reads as "auto".
pub fn decimate_ratio(target_faces: u32, headroom: f64, faces: u64) -> Option<f64> {
    if faces == 0 {
        return None;
    }
    let ratio = f64::from(target_faces) * headroom / faces as f64;
    (ratio < 1.0).then(|| ratio.max(MIN_DECIMATE))
}

/// The `TextureMesh` arguments for this config, run after `ReconstructMesh`:
/// with `dense.texture_decimate`, the ratio comes from the face count of
/// `out/mesh.ply`.
fn texture_mesh_step_args(out: &Path, config: &Config) -> Result<Vec<OsString>> {
    let dense = &config.dense;
    if !dense.texture_decimate {
        return Ok(texture_mesh_args(out, None));
    }
    let mesh = ply::element_counts(&out.join("mesh.ply"))?;
    let faces = mesh.get("face").copied().unwrap_or(0);
    let ratio = decimate_ratio(config.post.target_faces, dense.texture_headroom, faces);
    Ok(texture_mesh_args(out, ratio))
}

/// The last OpenMVS progress figure in `log`, as a fraction: OpenMVS redraws
/// `Estimated depth-maps 47 (38.84%, 9s, ETA 14s)...` with carriage returns.
/// Each pass of a step starts again from 0%.
pub fn last_percent(log: &str) -> Option<f64> {
    log.rmatch_indices("%,")
        .find_map(|(end, _)| {
            let head = &log[..end];
            let open = head.rfind('(')?;
            head[open + 1..].parse::<f64>().ok()
        })
        .map(|percent| percent / 100.0)
}

impl Stage for Dense {
    fn name(&self) -> &'static str {
        NAME
    }

    fn deps(&self) -> &[&'static str] {
        &[FRAMES, MASKS, SFM]
    }

    fn config(&self, config: &Config) -> serde_json::Value {
        let dense = serde_json::to_value(&config.dense).expect("dense config serialises to JSON");
        // With decimation on, the mesh depends on post.target_faces too.
        if config.dense.texture_decimate {
            serde_json::json!({ "dense": dense, "target_faces": config.post.target_faces })
        } else {
            dense
        }
    }

    fn version(&self, ctx: &RunContext) -> Result<String> {
        if ctx.config.dense.backend != OPENMVS {
            return Err(stage_error(format!(
                "dense.backend {:?} is not implemented (implemented: openmvs)",
                ctx.config.dense.backend
            )));
        }
        let colmap = ctx.tools.get(COLMAP)?.version(&["help"])?;
        let openmvs = ctx.tools.get(OPENMVS)?;
        if openmvs.spec.image.is_none() {
            return Err(stage_error(
                "the openmvs tool must run from its pinned image (ADR-0011)",
            ));
        }
        Ok(format!(
            "{NAME}/{REVISION} {colmap} {}",
            openmvs.version(&[])?
        ))
    }

    fn steps(&self, _: &Config) -> Vec<String> {
        let openmvs = openmvs_steps(Path::new("")).into_iter().map(|(log, _)| log);
        std::iter::once(undistort::LOG)
            .chain(openmvs)
            .map(str::to_owned)
            .collect()
    }

    fn progress(&self, step: &str, log: &str, _frames: Option<usize>) -> Option<f64> {
        match step {
            undistort::LOG => None,
            _ => last_percent(log),
        }
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
        let undistorted = out_dir.join("undistorted");
        let logs = out_dir.join("logs");
        let mount = |args| {
            Invocation::new(args)
                .mount(&images)
                .mount(&masks)
                .mount(&inputs[SFM].dir)
                .mount(out_dir)
        };

        let colmap = ctx.tools.get(COLMAP)?;
        let undistort_args = undistort::undistorter_args(&images, &sparse, &undistorted);
        let mut tools = vec![colmap.run(
            &mount(undistort_args),
            &logs.join(format!("{}.log", undistort::LOG)),
        )?];
        let distorted = Model::read_binary(&sparse)?;
        let model = Model::read_binary(&undistorted.join("sparse"))?;
        undistort::undistort_masks(&distorted, &model, &masks, &out_dir.join("masks"), |name| {
            format!("{}.mask.png", undistort::stem(name))
        })?;

        let openmvs = ctx.tools.get(OPENMVS)?;
        for (log, mut args) in openmvs_steps(out_dir) {
            // The ratio needs ReconstructMesh's face count, so it is set only now.
            if log == TEXTURE_MESH {
                args = texture_mesh_step_args(out_dir, &ctx.config)?;
            }
            tools.push(openmvs.run(&mount(args), &logs.join(format!("{log}.log")))?);
        }
        let textured = out_dir.join("mesh_textured.glb");
        if !textured.is_file() {
            return Err(stage_error(format!(
                "TextureMesh wrote no {} (see logs/TextureMesh.log)",
                textured.display()
            )));
        }

        let points = ply::element_counts(&out_dir.join("scene_dense.ply"))?;
        let mesh = ply::element_counts(&out_dir.join("mesh.ply"))?;
        let count = |m: &std::collections::BTreeMap<String, u64>, k: &str| {
            m.get(k).copied().unwrap_or(0) as f64
        };
        let metrics = [
            (DENSE_POINTS, count(&points, "vertex")),
            (MESH_VERTICES, count(&mesh, "vertex")),
            (MESH_FACES, count(&mesh, "face")),
        ]
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

    /// From `work/reposalmi-laajasalo/dense/79fd10672efc5a7c/logs/DensifyPointCloud.log`.
    const DENSIFY: &str = "\tthat 50504 points with 4+ views (73.64%)\n\
        Estimated depth-maps 46 (38.02%, 8s, ETA 14s)...\r\
        Estimated depth-maps 47 (38.84%, 9s, ETA 14s)...\rEstimated depth-maps 4";
    const FUSED: &str = "Dense fused depth-maps 120 (99.17%, 35s, ETA 293ms)...\r\
        Dense fused depth-maps 121 (100%, 35s300ms)           \n\
        13:31:00 [ScnDense] Depth-maps dense fused and filtered: 121 depth-maps, \
        66294213 depths, 2629388 points (4%%), 11.16 hits in 67.19 cached (35s332ms)\n";

    #[test]
    fn progress_is_the_last_openmvs_percentage() {
        assert_eq!(last_percent(DENSIFY), Some(0.3884));
        assert_eq!(last_percent(FUSED), Some(1.0));
        assert_eq!(last_percent("\t57059 points inside ROI (83.20%)\n"), None);
        assert_eq!(
            Dense.progress("DensifyPointCloud", DENSIFY, None),
            Some(0.3884)
        );
        assert_eq!(Dense.progress("image_undistorter", DENSIFY, None), None);
    }

    #[test]
    fn steps_are_the_log_names_in_run_order() {
        let config = Config::layered(
            None,
            &[],
            "[dataset]\nname='x'\ninput='x.mp4'".parse().unwrap(),
        )
        .unwrap();
        assert_eq!(
            Dense.steps(&config),
            [
                "image_undistorter",
                "InterfaceCOLMAP",
                "DensifyPointCloud",
                "ReconstructMesh",
                "TextureMesh"
            ]
        );
    }

    fn strings(args: &[OsString]) -> Vec<String> {
        args.iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn steps_densify_with_masks_then_mesh_and_texture_to_glb() {
        let steps = openmvs_steps(Path::new("/o"));
        let names: Vec<_> = steps.iter().map(|(n, _)| *n).collect();
        assert_eq!(
            names,
            [
                "InterfaceCOLMAP",
                "DensifyPointCloud",
                "ReconstructMesh",
                "TextureMesh"
            ]
        );
        for (name, args) in &steps {
            assert_eq!(&strings(args)[..3], [*name, "-w", "/o/"], "{name}");
        }
        let densify = strings(&steps[1].1);
        let tail: Vec<&str> = densify[3..].iter().map(String::as_str).collect();
        assert_eq!(
            tail,
            [
                "-i",
                "/o/scene.mvs",
                "-o",
                "/o/scene_dense.mvs",
                "--mask-path",
                "/o/masks",
                "--ignore-mask-label",
                "0",
                "--remove-dmaps",
                "1"
            ]
        );
        assert_eq!(
            &strings(&steps[3].1)[3..],
            [
                "-i",
                "/o/scene_dense.mvs",
                "-m",
                "/o/mesh.ply",
                "--export-type",
                "glb",
                "-o",
                "/o/mesh_textured.glb"
            ]
        );
    }

    #[test]
    fn texture_mesh_appends_decimate_only_with_a_ratio() {
        let plain = strings(&texture_mesh_args(Path::new("/o"), None));
        assert_eq!(plain, strings(&openmvs_steps(Path::new("/o"))[3].1));
        let decimated = strings(&texture_mesh_args(Path::new("/o"), Some(0.157049)));
        assert_eq!(decimated[..plain.len()], plain[..]);
        assert_eq!(decimated[plain.len()..], ["--decimate", "0.157049"]);
    }

    #[test]
    fn decimate_ratio_aims_at_the_target_times_headroom() {
        let ratio = decimate_ratio(500_000, 0.95, 3_024_735).unwrap();
        assert!((ratio - 475_000.0 / 3_024_735.0).abs() < 1e-12, "{ratio}");
        assert_eq!(decimate_ratio(500_000, 0.95, 475_000), None);
        assert_eq!(decimate_ratio(500_000, 0.95, 400_000), None);
        assert_eq!(
            decimate_ratio(500_000, 1.0, 500_001).map(|r| r < 1.0),
            Some(true)
        );
        assert_eq!(decimate_ratio(500_000, 0.95, 0), None);
        assert_eq!(
            decimate_ratio(1, 0.95, 1_000_000_000_000_000),
            Some(MIN_DECIMATE)
        );
    }

    #[test]
    fn texture_mesh_step_reads_mesh_faces_only_when_decimating() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("mesh.ply"),
            "ply\nformat binary_little_endian 1.0\nelement vertex 1500000\n\
             property float x\nelement face 2000000\n\
             property list uchar int vertex_indices\nend_header\n",
        )
        .unwrap();
        let configs = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../configs");
        let mut config = Config::load(&configs.join("reposalmi-laajasalo.toml")).unwrap();
        config.post.target_faces = 500_000;
        config.dense.texture_headroom = 0.8;
        let args = strings(&texture_mesh_step_args(dir.path(), &config).unwrap());
        assert_eq!(args[args.len() - 2..], ["--decimate", "0.200000"]);
        config.dense.texture_decimate = false;
        std::fs::remove_file(dir.path().join("mesh.ply")).unwrap();
        assert_eq!(
            strings(&texture_mesh_step_args(dir.path(), &config).unwrap()),
            strings(&texture_mesh_args(dir.path(), None))
        );
    }

    #[test]
    fn target_faces_is_in_the_config_only_when_decimating() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../configs");
        let a = Config::load(&dir.join("reposalmi-laajasalo.toml")).unwrap();
        let mut b = a.clone();
        b.post.target_faces += 1;
        assert_ne!(Dense.config(&a), Dense.config(&b));
        let (mut a, mut b) = (a, b);
        a.dense.texture_decimate = false;
        b.dense.texture_decimate = false;
        assert_eq!(Dense.config(&a), Dense.config(&b));
    }

    #[test]
    fn version_needs_the_openmvs_backend_and_image() {
        let image = "x/y@sha256:c1de51615dff33066cd305071226b6343c1168324ef3bd622f0825e55feb5efe";
        let table = format!(
            "[colmap]\nruntime = \"apptainer\"\nprogram = \"colmap\"\nimage = \"{image}\"\n\
             [openmvs]\nruntime = \"apptainer\"\nprogram = \"env\"\nimage = \"{image}\"\n"
        );
        let tools = shoreline_core::Tools::from_table(table.parse().unwrap()).unwrap();
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../configs");
        let mut config = Config::load(&dir.join("reposalmi-laajasalo.toml")).unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let ctx = RunContext::new(config.clone(), tools.clone(), tmp.path()).unwrap();
        assert_eq!(
            Dense.version(&ctx).unwrap(),
            format!("dense/1 {image} {image}")
        );
        config.dense.backend = "colmap".into();
        let ctx = RunContext::new(config, tools, tmp.path()).unwrap();
        assert!(Dense.version(&ctx).is_err());
    }
}
