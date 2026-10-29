//! SfM metrics and quality gates (ADR-0010), with COLMAP 4.2.1's definitions
//! (`Reconstruction::ComputeMeanTrackLength`, `ComputeMeanReprojectionError`).

use shoreline_core::stage::Metrics;
use shoreline_core::{Config, Error, Result};

use crate::model::Model;

/// Metric names as recorded in the manifest.
pub const REGISTERED_IMAGES: &str = "registered_images";
pub const INPUT_IMAGES: &str = "input_images";
pub const REGISTERED_RATIO: &str = "registered_ratio";
pub const POINTS: &str = "points";
pub const OBSERVATIONS: &str = "observations";
pub const MEAN_TRACK_LENGTH: &str = "mean_track_length";
pub const MEAN_REPROJ_ERROR_PX: &str = "mean_reproj_error_px";

/// Metrics of `model`, built from `input_images` frames. A model holds only
/// registered images, so the input count is the ratio's denominator.
pub fn metrics(model: &Model, input_images: usize) -> Metrics {
    let registered = model.images.len();
    let points = model.points3d.len();
    let observations: usize = model.images.values().map(|i| i.num_observations()).sum();
    let errors: Vec<f64> = model.points3d.values().filter_map(|p| p.error).collect();
    let ratio = |num: usize, den: usize| match den {
        0 => 0.0,
        _ => num as f64 / den as f64,
    };
    let mean_error = match errors.len() {
        0 => 0.0,
        n => errors.iter().sum::<f64>() / n as f64,
    };
    [
        (REGISTERED_IMAGES, registered as f64),
        (INPUT_IMAGES, input_images as f64),
        (REGISTERED_RATIO, ratio(registered, input_images)),
        (POINTS, points as f64),
        (OBSERVATIONS, observations as f64),
        (MEAN_TRACK_LENGTH, ratio(observations, points)),
        (MEAN_REPROJ_ERROR_PX, mean_error),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_owned(), v))
    .collect()
}

/// The v1 sfm gates (`[gates]`). Every failing gate is named in one error.
pub fn gate(config: &Config, metrics: &Metrics) -> Result<Vec<String>> {
    let g = &config.gates;
    let get = |key: &str| {
        metrics.get(key).copied().ok_or_else(|| Error::Stage {
            stage: crate::NAME,
            message: format!("metrics have no {key}"),
        })
    };
    let checks = [
        (
            REGISTERED_RATIO,
            get(REGISTERED_RATIO)?,
            "<",
            g.min_registered_ratio,
            "gates.min_registered_ratio",
        ),
        (
            MEAN_REPROJ_ERROR_PX,
            get(MEAN_REPROJ_ERROR_PX)?,
            ">",
            g.max_reproj_error_px,
            "gates.max_reproj_error_px",
        ),
        (
            MEAN_TRACK_LENGTH,
            get(MEAN_TRACK_LENGTH)?,
            "<",
            g.min_track_length,
            "gates.min_track_length",
        ),
    ];
    let failed: Vec<String> = checks
        .iter()
        .filter(|(_, value, op, limit, _)| match *op {
            "<" => value < limit,
            _ => value > limit,
        })
        .map(|(name, value, op, limit, key)| format!("{name} {value:.3} {op} {key} = {limit}"))
        .collect();
    match failed.is_empty() {
        true => Ok(Vec::new()),
        false => Err(Error::Gate(failed.join("; "))),
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;
    use crate::model::tests::sample;

    fn repo_config() -> Config {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../configs");
        Config::load(&dir.join("reposalmi-laajasalo.toml")).unwrap()
    }

    #[test]
    fn metrics_follow_colmap_definitions() {
        let m = metrics(&sample(), 4);
        assert_eq!(m[REGISTERED_IMAGES], 2.0);
        assert_eq!(m[INPUT_IMAGES], 4.0);
        assert_eq!(m[REGISTERED_RATIO], 0.5);
        assert_eq!(m[POINTS], 3.0);
        // 3 observations in image 1 and 2 in image 2; the unmatched keypoint is not one.
        assert_eq!(m[OBSERVATIONS], 5.0);
        assert_eq!(m[MEAN_TRACK_LENGTH], 5.0 / 3.0);
        // Plain mean over points with an error; point 3's unset error is skipped.
        assert_eq!(m[MEAN_REPROJ_ERROR_PX], 0.75);
    }

    #[test]
    fn empty_model_has_zero_metrics() {
        let m = metrics(&Model::default(), 0);
        assert!(m.values().all(|&v| v == 0.0), "{m:?}");
    }

    #[test]
    fn gates_pass_at_their_limits() {
        let config = repo_config();
        let g = &config.gates;
        let m: Metrics = [
            (REGISTERED_RATIO, g.min_registered_ratio),
            (MEAN_REPROJ_ERROR_PX, g.max_reproj_error_px),
            (MEAN_TRACK_LENGTH, g.min_track_length),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_owned(), v))
        .collect();
        assert!(gate(&config, &m).unwrap().is_empty());
    }

    #[test]
    fn failing_gates_are_all_named_in_one_error() {
        let config = repo_config();
        // 2 of 4 registered, mean track 1.67: two gates fail, reprojection passes.
        let err = gate(&config, &metrics(&sample(), 4)).unwrap_err();
        let Error::Gate(message) = &err else {
            panic!("expected a gate error, got {err:?}");
        };
        assert!(message.contains("gates.min_registered_ratio"), "{message}");
        assert!(message.contains("gates.min_track_length"), "{message}");
        assert!(!message.contains("gates.max_reproj_error_px"), "{message}");
    }

    #[test]
    fn missing_metric_is_a_stage_error() {
        let err = gate(&repo_config(), &Metrics::new()).unwrap_err();
        assert!(matches!(&err, Error::Stage { message, .. } if message.contains(REGISTERED_RATIO)));
    }

    /// A COLMAP run directory (`SHORELINE_TEST_SFM_DIR`: `sparse/0` and the
    /// `model_analyzer.txt` written for it) against COLMAP's own figures.
    /// `SHORELINE_TEST_SFM_FRAMES` is the number of input frames (default: the
    /// registered images, a full registration).
    mod integration {
        use super::*;

        fn analyzer_value(text: &str, label: &str) -> f64 {
            text.lines()
                .find_map(|l| l.split_once(&format!("] {label}: ")))
                .map(|(_, v)| v.trim_end_matches("px").parse().unwrap())
                .unwrap_or_else(|| panic!("no {label} in model_analyzer.txt"))
        }

        #[test]
        fn model_reproduces_model_analyzer() {
            let run = std::path::PathBuf::from(
                std::env::var_os("SHORELINE_TEST_SFM_DIR")
                    .expect("SHORELINE_TEST_SFM_DIR is not set"),
            );
            let text = std::fs::read_to_string(run.join("model_analyzer.txt")).unwrap();
            let model = Model::read_binary(&run.join("sparse/0")).unwrap();
            let frames = match std::env::var("SHORELINE_TEST_SFM_FRAMES") {
                Ok(n) => n.parse().unwrap(),
                Err(_) => analyzer_value(&text, "Registered images") as usize,
            };
            let m = metrics(&model, frames);
            let close = |a: f64, b: f64| (a - b).abs() < 5e-7;
            assert_eq!(
                m[REGISTERED_IMAGES],
                analyzer_value(&text, "Registered images")
            );
            assert_eq!(m[POINTS], analyzer_value(&text, "Points"));
            assert_eq!(m[OBSERVATIONS], analyzer_value(&text, "Observations"));
            let track = analyzer_value(&text, "Mean track length");
            assert!(
                close(m[MEAN_TRACK_LENGTH], track),
                "{} vs {track}",
                m[MEAN_TRACK_LENGTH]
            );
            let error = analyzer_value(&text, "Mean reprojection error");
            assert!(
                close(m[MEAN_REPROJ_ERROR_PX], error),
                "{} vs {error}",
                m[MEAN_REPROJ_ERROR_PX]
            );
            assert_eq!(model.cameras.len(), 1);
            assert!(gate(&repo_config(), &m).unwrap().is_empty());
        }
    }
}
