//! `masks` stage: per-image masks of water, sky and transient objects
//! (ADR-0006, ADR-0007).
//!
//! The `segment` tool (the pinned container image) writes raw class masks;
//! this stage dilates them by `masks.dilate_px` with OpenCV's elliptical
//! kernel and writes them in the COLMAP convention: `images/00001.jpg` →
//! `masks/00001.jpg.png`, black = ignore. Output: `masks/`, `raw/` (the
//! tool's undilated masks), `overlays/` (every 12th frame, for review),
//! `masks.csv` (each frame's masked fraction) and `logs/segment.log`.

pub mod dilate;

use std::ffi::OsString;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use shoreline_core::error::io_error;
use shoreline_core::stage::{Inputs, Metrics, StageRun};
use shoreline_core::tools::Invocation;
use shoreline_core::{Config, Error, Result, RunContext, Stage};

pub const NAME: &str = "masks";
/// Log stem of the segmentation step.
const LOG: &str = "segment";
/// The stage whose `images/` this stage masks (ADR-0005).
const FRAMES: &str = "frames";

/// Bump when the output changes for the same config, frames and tool.
const REVISION: u32 = 1;
/// One review overlay per this many frames, as in the M0 spike.
const OVERLAY_EVERY: usize = 12;
/// Overlays are scaled by this factor.
const OVERLAY_SCALE: f64 = 0.4;

pub struct Masks;

/// One frame's share of ignored pixels after dilation.
#[derive(Debug, Clone, PartialEq)]
pub struct Coverage {
    pub image: String,
    pub masked_fraction: f64,
}

/// Frames segmented so far: the segment tool prints `<frame>.jpg <score>`
/// as it finishes each frame.
pub fn segmented(log: &str) -> usize {
    log.lines()
        .filter(|line| {
            line.split_once(' ').is_some_and(|(frame, score)| {
                frame.ends_with(".jpg") && score.trim().parse::<f64>().is_ok()
            })
        })
        .count()
}

/// Mask path for `image` (relative to the image root) under `mask_root`.
/// Subdirectories are preserved.
pub fn mask_path(mask_root: &Path, image: &Path) -> PathBuf {
    let mut name = image.as_os_str().to_owned();
    name.push(".png");
    mask_root.join(name)
}

impl Stage for Masks {
    fn name(&self) -> &'static str {
        NAME
    }

    fn deps(&self) -> &[&'static str] {
        &[FRAMES]
    }

    fn config(&self, config: &Config) -> serde_json::Value {
        serde_json::to_value(&config.masks).expect("masks config serialises to JSON")
    }

    /// The pinned image reference: the weights live in the image (ADR-0006),
    /// so a native `segment` has no version that identifies them.
    fn version(&self, ctx: &RunContext) -> Result<String> {
        let segment = ctx.tools.get("segment")?;
        if segment.spec.image.is_none() {
            return Err(stage_error(
                "the segment tool must run from its pinned container image (ADR-0006); \
                 a native segment has no version that identifies its weights",
            ));
        }
        Ok(format!("{NAME}/{REVISION} {}", segment.version(&[])?))
    }

    fn steps(&self, _: &Config) -> Vec<String> {
        vec![LOG.to_owned()]
    }

    fn progress(&self, _step: &str, log: &str, frames: Option<usize>) -> Option<f64> {
        let frames = frames.filter(|&n| n > 0)?;
        Some((segmented(log) as f64 / frames as f64).min(1.0))
    }

    fn run(
        &self,
        ctx: &RunContext,
        inputs: &Inputs,
        out_dir: &Path,
        on_event: &mut dyn FnMut(shoreline_core::pipeline::Event<'_>),
    ) -> Result<StageRun> {
        let images = inputs[FRAMES].dir.join("images");
        let (raw, masks, overlays) = (
            out_dir.join("raw"),
            out_dir.join("masks"),
            out_dir.join("overlays"),
        );
        for dir in [&raw, &masks, &overlays] {
            std::fs::create_dir_all(dir).map_err(io_error(dir))?;
        }

        let args: Vec<OsString> = vec![
            "--classes".into(),
            ctx.config.masks.classes.join(",").into(),
            images.as_os_str().into(),
            raw.as_os_str().into(),
        ];
        let segment = Invocation::new(args).mount(&images).mount(out_dir);
        let record = ctx
            .tools
            .get("segment")?
            .run(&segment, &out_dir.join(format!("logs/{LOG}.log")))?;
        on_event(shoreline_core::pipeline::Event::Step {
            stage: NAME,
            message: "segmented frames",
        });

        let dilate_px = ctx.config.masks.dilate_px as usize;
        let mut rows = Vec::new();
        for (i, name) in frame_names(&images)?.into_iter().enumerate() {
            let image = images.join(&name);
            let raw_mask = mask_path(&raw, Path::new(&name));
            if !raw_mask.is_file() {
                return Err(stage_error(format!(
                    "segment wrote no mask for {name} (expected {})",
                    raw_mask.display()
                )));
            }
            let (class, w, h) = read_mask(&raw_mask)?;
            let (iw, ih) = image::image_dimensions(&image).map_err(|e| image_error(&image, e))?;
            if (iw as usize, ih as usize) != (w, h) {
                return Err(stage_error(format!(
                    "{} is {w}x{h} but frame {name} is {iw}x{ih}",
                    raw_mask.display()
                )));
            }
            let ignore = dilate::dilate_ellipse(&class, w, h, dilate_px);
            write_colmap_mask(&mask_path(&masks, Path::new(&name)), &ignore, w, h)?;
            if i % OVERLAY_EVERY == 0 {
                write_overlay(&image, &ignore, &overlays.join(&name))?;
            }
            let masked = ignore.iter().filter(|&&set| set).count() as f64 / (w * h) as f64;
            rows.push(Coverage {
                image: name,
                masked_fraction: masked,
            });
        }
        on_event(shoreline_core::pipeline::Event::Step {
            stage: NAME,
            message: &format!("processed {} frames", rows.len()),
        });
        let csv = out_dir.join("masks.csv");
        std::fs::write(&csv, coverage_csv(&rows)).map_err(io_error(&csv))?;
        Ok(StageRun {
            tools: vec![record],
            metrics: metrics(&rows),
        })
    }

    /// Warn-only (kickoff § Quality gates): a frame whose masked fraction
    /// exceeds `gates.max_masked_fraction` may be over-masked.
    fn gate(&self, config: &Config, metrics: &Metrics) -> Result<Vec<String>> {
        let limit = config.gates.max_masked_fraction;
        let max = *metrics
            .get("masked_max")
            .ok_or_else(|| stage_error("metrics have no masked_max"))?;
        Ok(match max > limit {
            true => vec![format!(
                "highest masked fraction {max:.3} exceeds gates.max_masked_fraction = {limit} \
                 (per-frame values in masks.csv)"
            )],
            false => Vec::new(),
        })
    }
}

/// Load an 8-bit mask PNG as `(set pixels, width, height)`, set where > 0.
pub fn read_mask(path: &Path) -> Result<(Vec<bool>, usize, usize)> {
    let img = image::open(path)
        .map_err(|e| image_error(path, e))?
        .into_luma8();
    let (w, h) = img.dimensions();
    let set = img.into_raw().into_iter().map(|v| v > 0).collect();
    Ok((set, w as usize, h as usize))
}

/// Write `ignore` as a COLMAP mask: 255 = keep, 0 = ignore.
pub fn write_colmap_mask(path: &Path, ignore: &[bool], width: usize, height: usize) -> Result<()> {
    let pixels = ignore.iter().map(|&i| if i { 0 } else { 255 }).collect();
    let img = image::GrayImage::from_raw(width as u32, height as u32, pixels)
        .expect("mask has width x height pixels");
    img.save(path).map_err(|e| image_error(path, e))
}

/// The frame with ignored pixels tinted magenta, scaled by `OVERLAY_SCALE`.
pub fn write_overlay(image: &Path, ignore: &[bool], out: &Path) -> Result<()> {
    let mut rgb = image::open(image)
        .map_err(|e| image_error(image, e))?
        .into_rgb8();
    assert_eq!(rgb.pixels().len(), ignore.len(), "overlay mask size");
    let tint = |v: u8, t: f64| (0.4 * f64::from(v) + 0.6 * t).round() as u8;
    for (p, _) in rgb.pixels_mut().zip(ignore).filter(|(_, i)| **i) {
        let [r, g, b] = p.0;
        p.0 = [tint(r, 255.0), tint(g, 0.0), tint(b, 255.0)];
    }
    let (w, h) = rgb.dimensions();
    let scale = |v: u32| ((f64::from(v) * OVERLAY_SCALE).round() as u32).max(1);
    let scaled = image::imageops::resize(
        &rgb,
        scale(w),
        scale(h),
        image::imageops::FilterType::Triangle,
    );
    scaled.save(out).map_err(|e| image_error(out, e))
}

/// `image,masked_fraction` rows, full precision.
pub fn coverage_csv(rows: &[Coverage]) -> String {
    let mut csv = String::from("image,masked_fraction\n");
    for row in rows {
        writeln!(csv, "{},{}", row.image, row.masked_fraction).expect("writing to a String");
    }
    csv
}

/// `frames`, `masked_mean` and `masked_max`.
pub fn metrics(rows: &[Coverage]) -> Metrics {
    let n = rows.len() as f64;
    let fractions = rows.iter().map(|r| r.masked_fraction);
    [
        ("frames", n),
        ("masked_mean", fractions.clone().sum::<f64>() / n),
        ("masked_max", fractions.fold(0.0, f64::max)),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_owned(), v))
    .collect()
}

/// `*.jpg` names in `images`, in frame order, failing when there are none.
fn frame_names(images: &Path) -> Result<Vec<String>> {
    let mut names = Vec::new();
    for entry in std::fs::read_dir(images).map_err(io_error(images))? {
        let name = entry.map_err(io_error(images))?.file_name();
        if let Some(name) = name.to_str().filter(|n| n.ends_with(".jpg")) {
            names.push(name.to_owned());
        }
    }
    names.sort_by(|a, b| (a.len(), a).cmp(&(b.len(), b)));
    if names.is_empty() {
        return Err(stage_error(format!("no frames in {}", images.display())));
    }
    Ok(names)
}

fn image_error(path: &Path, e: image::ImageError) -> Error {
    stage_error(format!("{}: {e}", path.display()))
}

fn stage_error(message: impl Into<String>) -> Error {
    Error::Stage {
        stage: NAME,
        message: message.into(),
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    /// From `work/reposalmi-laajasalo/masks/*/logs/segment.log`.
    const SEGMENT: &str = "- MISSING:\tthose params were newly initialized because missing from \
        the checkpoint. Consider training on your downstream task.\n\
        00001.jpg 0.407792\n00002.jpg 0.409480\n00003.jpg 0.421299\n000";

    #[test]
    fn progress_counts_segmented_frames() {
        assert_eq!(segmented(SEGMENT), 3);
        assert_eq!(Masks.progress("segment", SEGMENT, Some(12)), Some(0.25));
        assert_eq!(Masks.progress("segment", SEGMENT, Some(0)), None);
        assert_eq!(Masks.progress("segment", SEGMENT, None), None);
    }
    use shoreline_core::Tools;
    use shoreline_core::stage::StageOutput;

    fn repo_config() -> Config {
        Config::load(
            &Path::new(env!("CARGO_MANIFEST_DIR")).join("../../configs/reposalmi-laajasalo.toml"),
        )
        .unwrap()
    }

    fn fixture(dir: &str, name: &str) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures")
            .join(dir)
            .join(name)
    }

    #[test]
    fn appends_png_to_full_file_name() {
        assert_eq!(
            mask_path(Path::new("masks"), Path::new("00001.jpg")),
            Path::new("masks/00001.jpg.png")
        );
    }

    #[test]
    fn preserves_subdirectories() {
        assert_eq!(
            mask_path(Path::new("masks"), Path::new("cam0/00001.JPG")),
            Path::new("masks/cam0/00001.JPG.png")
        );
    }

    #[test]
    fn config_is_the_masks_section_and_tracks_every_field() {
        let config = repo_config();
        let base = Masks.config(&config);
        assert_eq!(base, serde_json::to_value(&config.masks).unwrap());
        let changes: [fn(&mut shoreline_core::config::Masks); 2] =
            [|m| m.classes.push("tree".into()), |m| m.dilate_px = 4];
        for change in changes {
            let mut changed = config.clone();
            change(&mut changed.masks);
            assert_ne!(Masks.config(&changed), base);
        }
    }

    /// An executable stand-in for the segment tool: records its arguments in
    /// `args.txt` and writes the raw fixture mask for every frame except those
    /// named in `skip`. Like the real tool, it has no `--version`.
    fn stub_segment(dir: &Path, skip: &[&str]) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let raw = fixture("masks", "raw_320x240.png");
        let skips: String = skip.iter().map(|s| format!(" {s}")).collect();
        let script = format!(
            "#!/bin/sh\n\
             printf '%s\\n' \"$@\" > '{args}'\n\
             mkdir -p \"$4\"\n\
             for f in \"$3\"/*.jpg; do\n\
               n=$(basename \"$f\")\n\
               case \"{skips} \" in *\" $n \"*) continue;; esac\n\
               cp '{raw}' \"$4/$n.png\"\n\
             done\n",
            args = dir.join("args.txt").display(),
            raw = raw.display(),
        );
        let path = dir.join("segment-stub");
        fs::write(&path, script).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        assert_ne!(fs::metadata(&path).unwrap().permissions().mode() & 0o111, 0);
        path
    }

    /// A context whose segment tool is the stub, and frames output holding
    /// copies of the named frames fixtures as 00001.jpg, 00002.jpg, …
    fn setup(dir: &Path, frames: &[&str], skip: &[&str]) -> (RunContext, Inputs, PathBuf) {
        setup_with(dir, repo_config(), frames, skip)
    }

    fn setup_with(
        dir: &Path,
        config: Config,
        frames: &[&str],
        skip: &[&str],
    ) -> (RunContext, Inputs, PathBuf) {
        let program = stub_segment(dir, skip);
        let table = format!(
            "[segment]\nruntime = \"native\"\nprogram = \"{}\"\n",
            program.display()
        );
        let tools = Tools::from_table(table.parse().unwrap()).unwrap();
        let ctx = RunContext::new(config, tools, &dir.join("work")).unwrap();
        let frames_dir = dir.join("frames-out");
        fs::create_dir_all(frames_dir.join("images")).unwrap();
        for (i, name) in frames.iter().enumerate() {
            let dst = frames_dir.join("images").join(format!("{:05}.jpg", i + 1));
            fs::copy(fixture("frames", &format!("{name}.jpg")), dst).unwrap();
        }
        let fp = shoreline_core::Fingerprint::compute(FRAMES, &0, &[], "");
        let inputs = Inputs::from([(
            FRAMES,
            StageOutput {
                fingerprint: fp,
                dir: frames_dir,
            },
        )]);
        let out = dir.join("out");
        fs::create_dir(&out).unwrap();
        (ctx, inputs, out)
    }

    #[test]
    fn version_is_the_revision_and_the_image_reference() {
        let dir = tempfile::tempdir().unwrap();
        let image =
            "ghcr.io/x/seg@sha256:0000000000000000000000000000000000000000000000000000000000000000";
        let table = format!(
            "[segment]\nruntime = \"docker\"\nprogram = \"segment\"\nimage = \"{image}\"\n"
        );
        let tools = Tools::from_table(table.parse().unwrap()).unwrap();
        let ctx = RunContext::new(repo_config(), tools, dir.path()).unwrap();
        assert_eq!(Masks.version(&ctx).unwrap(), format!("masks/1 {image}"));
    }

    #[test]
    fn a_native_segment_tool_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let (ctx, _, _) = setup(dir.path(), &[], &[]);
        let err = Masks.version(&ctx).unwrap_err().to_string();
        assert!(
            err.contains("container image") && err.contains("ADR-0006"),
            "{err}"
        );
    }

    #[test]
    fn run_emits_step_events_with_collecting_sink() {
        let dir = tempfile::tempdir().unwrap();
        let (ctx, inputs, out) = setup(dir.path(), &["sharp", "soft", "near_keep"], &[]);
        let mut events: Vec<(String, String)> = Vec::new();
        let run = Masks
            .run(&ctx, &inputs, &out, &mut |ev| {
                if let shoreline_core::pipeline::Event::Step { stage, message } = ev {
                    events.push((stage.to_string(), message.to_string()));
                }
            })
            .unwrap();

        assert_eq!(run.metrics["frames"], 3.0);
        assert_eq!(
            events,
            vec![
                ("masks".to_string(), "segmented frames".to_string()),
                ("masks".to_string(), "processed 3 frames".to_string()),
            ]
        );
    }

    #[test]
    fn dilation_follows_masks_dilate_px() {
        let dir = tempfile::tempdir().unwrap();
        let mut config = repo_config();
        config.masks.dilate_px = 3;
        let (ctx, inputs, out) = setup_with(dir.path(), config, &["sharp"], &[]);
        let run = Masks.run(&ctx, &inputs, &out, &mut |_| {}).unwrap();
        let mask = image::open(out.join("masks/00001.jpg.png"))
            .unwrap()
            .into_luma8();
        assert_eq!(mask.get_pixel(5, 82).0[0], 0, "raw rows 0..80 grown by 3");
        assert_eq!(mask.get_pixel(5, 83).0[0], 255);
        let expected = 83.0 * 320.0 / (320.0 * 240.0);
        assert!((run.metrics["masked_mean"] - expected).abs() < 1e-12);
    }

    #[test]
    fn run_writes_dilated_colmap_masks_overlays_and_coverage() {
        let dir = tempfile::tempdir().unwrap();
        let (ctx, inputs, out) = setup(dir.path(), &["sharp", "soft", "near_keep"], &[]);
        let run = Masks.run(&ctx, &inputs, &out, &mut |_| {}).unwrap();

        let args = fs::read_to_string(dir.path().join("args.txt")).unwrap();
        let classes = ctx.config.masks.classes.join(",");
        assert!(
            args.starts_with(&format!("--classes\n{classes}\n")),
            "{args}"
        );

        // Raw: top 80 rows set. Dilated by 8 px: rows 0..88 ignored (0), the rest kept (255).
        let mask = image::open(out.join("masks/00001.jpg.png"))
            .unwrap()
            .into_luma8();
        assert_eq!(mask.dimensions(), (320, 240));
        assert_eq!(mask.get_pixel(5, 87).0[0], 0, "within the dilation");
        assert_eq!(mask.get_pixel(5, 88).0[0], 255, "beyond the dilation");
        let values: std::collections::BTreeSet<u8> = mask.pixels().map(|p| p.0[0]).collect();
        assert_eq!(values, [0, 255].into());

        let expected = 88.0 * 320.0 / (320.0 * 240.0);
        assert!(
            (run.metrics["masked_mean"] - expected).abs() < 1e-12,
            "{:?}",
            run.metrics
        );
        assert_eq!(run.metrics["frames"], 3.0);
        assert!(out.join("overlays/00001.jpg").is_file());
        assert!(
            !out.join("overlays/00002.jpg").exists(),
            "every 12th frame only"
        );
        let overlay = image::image_dimensions(out.join("overlays/00001.jpg")).unwrap();
        assert_eq!(overlay, (128, 96));
        let csv = fs::read_to_string(out.join("masks.csv")).unwrap();
        assert_eq!(csv.lines().count(), 4, "{csv}");
        assert_eq!(run.tools[0].log, out.join("logs/segment.log"));
        assert_eq!(Masks.steps(&ctx.config), ["segment"]);

        // Overlay content (128x96): mean red-minus-green over a row range.
        let overlay = image::open(out.join("overlays/00001.jpg"))
            .unwrap()
            .into_rgb8();
        let red_minus_green = |rows: std::ops::Range<u32>| {
            let px: Vec<f64> = rows
                .flat_map(|y| (10..118).map(move |x| (x, y)))
                .map(|(x, y)| {
                    let [r, g, _] = overlay.get_pixel(x, y).0;
                    f64::from(r) - f64::from(g)
                })
                .collect();
            px.iter().sum::<f64>() / px.len() as f64
        };
        assert!(
            red_minus_green(5..25) > 100.0,
            "ignored rows are tinted magenta"
        );
        assert!(red_minus_green(60..90) < 40.0, "kept rows are not tinted");
        // Overlay row 33 samples source rows ~81-86: in the dilation band, not the raw mask.
        assert!(
            red_minus_green(33..34) > 100.0,
            "the overlay shows the dilated mask"
        );
    }

    #[test]
    fn a_frame_without_a_raw_mask_is_an_error_naming_it() {
        let dir = tempfile::tempdir().unwrap();
        let (ctx, inputs, out) = setup(dir.path(), &["sharp", "soft"], &["00002.jpg"]);
        let err = Masks
            .run(&ctx, &inputs, &out, &mut |_| {})
            .unwrap_err()
            .to_string();
        assert!(err.contains("segment wrote no mask for 00002.jpg"), "{err}");
    }

    #[test]
    fn a_raw_mask_of_the_wrong_size_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let (ctx, inputs, out) = setup(dir.path(), &["odd"], &[]);
        let err = Masks
            .run(&ctx, &inputs, &out, &mut |_| {})
            .unwrap_err()
            .to_string();
        assert!(err.contains("320x240") && err.contains("37x23"), "{err}");
    }

    #[test]
    fn gate_warns_above_the_masked_fraction_limit() {
        let config = repo_config();
        let limit = config.gates.max_masked_fraction;
        let metrics = |max: f64| -> Metrics {
            [
                ("masked_max".to_owned(), max),
                ("masked_mean".to_owned(), 0.4),
            ]
            .into()
        };
        assert!(Masks.gate(&config, &metrics(limit)).unwrap().is_empty());
        let warnings = Masks.gate(&config, &metrics(limit + 0.05)).unwrap();
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("max_masked_fraction"), "{warnings:?}");
    }

    #[test]
    fn csv_and_metrics() {
        let rows = [
            Coverage {
                image: "00001.jpg".into(),
                masked_fraction: 0.25,
            },
            Coverage {
                image: "00002.jpg".into(),
                masked_fraction: 0.75,
            },
        ];
        assert_eq!(
            coverage_csv(&rows),
            "image,masked_fraction\n00001.jpg,0.25\n00002.jpg,0.75\n"
        );
        let m = metrics(&rows);
        assert_eq!(
            (m["frames"], m["masked_mean"], m["masked_max"]),
            (2.0, 0.5, 0.75)
        );
    }

    /// Need Docker and the pinned segment image: `cargo nextest run --profile integration`.
    mod integration {
        use super::*;

        #[test]
        fn the_pinned_image_masks_a_frame() {
            let dir = tempfile::tempdir().unwrap();
            let configs = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../configs");
            let dataset = configs.join("reposalmi-laajasalo.toml");
            let tools = Tools::load(&dataset, None).unwrap();
            let ctx = RunContext::new(repo_config(), tools, &dir.path().join("work")).unwrap();
            let (_, inputs, out) = setup(dir.path(), &["sharp"], &[]);
            let run = Masks.run(&ctx, &inputs, &out, &mut |_| {}).unwrap();
            assert_eq!(run.metrics["frames"], 1.0);
            assert!(out.join("masks/00001.jpg.png").is_file());
            assert!(
                run.tools[0]
                    .argv
                    .iter()
                    .any(|a| a.contains("shoreline-segmentation@sha256:"))
            );
        }
    }
}
