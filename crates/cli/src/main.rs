use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use shoreline_core::pipeline::{self, Event, StageResult};
use shoreline_core::{Config, RunContext, Stage, Tools, dag, dirs};
use shoreline_dense::Dense;
use shoreline_export::Export;
use shoreline_frames::Frames;
use shoreline_masks::Masks;
use shoreline_post::Post;
use shoreline_sfm::Sfm;
use shoreline_splat::Splat;
use shoreline_video::VideoStage;

/// Water-aware photogrammetry and Gaussian splatting pipeline.
#[derive(Debug, Parser)]
#[command(version)]
struct Cli {
    /// Host tools file; its tables replace those of every other tools layer.
    #[arg(long, global = true, env = "SHORELINE_TOOLS")]
    tools: Option<PathBuf>,
    /// Root of the stage cache: outputs go to <work>/<dataset>/<stage>/<fingerprint>/.
    #[arg(long, global = true, env = "SHORELINE_WORK", default_value = "work")]
    work: PathBuf,
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Run the implemented stages for a dataset config. Relative paths in the
    /// config are resolved against the working directory.
    Run {
        config: PathBuf,
        /// Run only this stage and what it depends on (repeatable).
        #[arg(long = "stage")]
        stages: Vec<String>,
    },
    /// Print the resolved config: built-in defaults, the user's config.toml,
    /// defaults.toml beside the config, the config. Without a config, the
    /// defaults with a placeholder dataset.
    Config { config: Option<PathBuf> },
    /// Print the stage execution order.
    Plan { config: Option<PathBuf> },
    /// Print the resolved tools: built-in, the user's tools.toml, tools.toml
    /// beside the config, then --tools.
    Tools { config: Option<PathBuf> },
}

/// The stages for `config`, in pipeline order; `run` refuses targets outside
/// this list. Export depends on the video stage only with `export.videos`.
fn stages(config: &Config) -> Vec<&'static dyn Stage> {
    vec![
        &Frames,
        &Masks,
        &Sfm,
        &Dense,
        &Splat,
        &Post,
        &VideoStage,
        Export::for_config(config),
    ]
}

/// What `run` builds when no `--stage` is given: the deliverables.
const DEFAULT_TARGETS: &[&str] = &["export"];

/// Placeholder dataset for commands run without a config.
const PLACEHOLDER_DATASET: &str = "[dataset]\nname = \"example\"\ninput = \"example.mp4\"\n";

fn load(path: Option<&Path>) -> Result<Config> {
    let user = dirs::config_dir();
    match path {
        Some(path) => Config::layered(user.as_deref(), &[path], toml::Table::new())
            .with_context(|| format!("loading {}", path.display())),
        None => Ok(Config::layered(
            user.as_deref(),
            &[],
            PLACEHOLDER_DATASET.parse()?,
        )?),
    }
}

fn load_tools(config: Option<&Path>, overlay: Option<&Path>) -> Result<Tools> {
    Tools::layered(dirs::config_dir().as_deref(), config, overlay).context("loading tools")
}

/// Refuse targets that are not implemented, telling a planned stage from a typo.
fn check_targets(targets: &[&str], implemented: &[&str]) -> Result<()> {
    let planned: Vec<&str> = dag::PIPELINE.iter().map(|(name, _)| *name).collect();
    for target in targets {
        if implemented.contains(target) {
            continue;
        }
        if planned.contains(target) {
            bail!(
                "stage {target} is not implemented yet (implemented: {})",
                implemented.join(", ")
            );
        }
        bail!("unknown stage {target} (stages: {})", planned.join(", "));
    }
    Ok(())
}

/// `dataset.input` must exist; relative paths resolve against `cwd`.
fn check_input(input: &Path, cwd: &Path) -> Result<()> {
    if !input.exists() {
        bail!(
            "dataset.input {} does not exist (relative paths resolve against the working \
             directory, {})",
            input.display(),
            cwd.display()
        );
    }
    Ok(())
}

fn report(event: Event<'_>) {
    match event {
        Event::Discarded { stage, dir } => {
            eprintln!("{stage}: discarded interrupted output {}", dir.display());
        }
        Event::Running { stage, dir } => eprintln!("{stage}: running in {}", dir.display()),
        Event::Finished(r) if r.cached => eprintln!("{}: cached", r.stage),
        Event::Finished(r) => eprintln!("{}: done", r.stage),
        Event::Warning { stage, message } => eprintln!("{stage}: warning: {message}"),
    }
}

/// `stage<TAB>ran|cached<TAB>dir<TAB>metric=value ...`
fn summary(result: &StageResult) -> String {
    let status = if result.cached { "cached" } else { "ran" };
    let metrics: Vec<String> = result
        .metrics
        .iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect();
    format!(
        "{}\t{status}\t{}\t{}",
        result.stage,
        result.dir.display(),
        metrics.join(" ")
    )
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Config { config } => {
            print!("{}", toml::to_string_pretty(&load(config.as_deref())?)?);
        }
        Command::Plan { config } => {
            let config = load(config.as_deref())?;
            for (i, stage) in pipeline::plan(&stages(&config), DEFAULT_TARGETS)?
                .iter()
                .enumerate()
            {
                println!("{} {stage}", i + 1);
            }
        }
        Command::Tools { config } => {
            let tools = load_tools(config.as_deref(), cli.tools.as_deref())?;
            print!("{}", toml::to_string_pretty(&tools)?);
        }
        Command::Run { config, stages } => {
            let tools = load_tools(Some(&config), cli.tools.as_deref())?;
            let ctx = RunContext::new(load(Some(&config))?, tools, &cli.work)?;
            let mut targets: Vec<&str> = stages.iter().map(String::as_str).collect();
            let all = self::stages(&ctx.config);
            let implemented: Vec<&str> = all.iter().map(|s| s.name()).collect();
            check_targets(&targets, &implemented)?;
            if targets.is_empty() {
                targets = DEFAULT_TARGETS.to_vec();
            }
            check_input(&ctx.config.dataset.input, &std::env::current_dir()?)?;
            for result in pipeline::run(&ctx, &all, &targets, &mut report)? {
                println!("{}", summary(&result));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn graph(config: &Config) -> Vec<(&'static str, Vec<&'static str>)> {
        stages(config)
            .iter()
            .map(|s| (s.name(), s.deps().to_vec()))
            .collect()
    }

    #[test]
    fn stages_match_the_pipeline_graph() {
        let mut config = Config::load(
            &Path::new(env!("CARGO_MANIFEST_DIR")).join("../../configs/reposalmi-laajasalo.toml"),
        )
        .unwrap();
        config.export.videos = false;
        let mut expected: Vec<_> = dag::PIPELINE
            .iter()
            .map(|(n, d)| (*n, d.to_vec()))
            .collect();
        assert_eq!(graph(&config), expected);
        assert_eq!(
            pipeline::plan(&stages(&config), DEFAULT_TARGETS).unwrap(),
            ["frames", "masks", "sfm", "dense", "splat", "post", "export"]
        );

        config.export.videos = true;
        expected.last_mut().unwrap().1.push("video");
        assert_eq!(graph(&config), expected);
        assert_eq!(
            pipeline::plan(&stages(&config), DEFAULT_TARGETS).unwrap(),
            [
                "frames", "masks", "sfm", "dense", "splat", "post", "video", "export"
            ]
        );
    }

    #[test]
    fn config_plan_and_tools_take_an_optional_config() {
        for cmd in ["config", "plan", "tools"] {
            Cli::try_parse_from(["shoreline-recon", cmd]).unwrap();
            Cli::try_parse_from(["shoreline-recon", cmd, "c.toml"]).unwrap();
        }
        assert!(Cli::try_parse_from(["shoreline-recon", "run"]).is_err());
    }

    #[test]
    fn placeholder_dataset_loads_over_the_built_in_defaults() {
        let cfg = Config::layered(None, &[], PLACEHOLDER_DATASET.parse().unwrap()).unwrap();
        assert_eq!(cfg.dataset.name, "example");
    }

    #[test]
    fn targets_tell_planned_stages_from_typos() {
        check_targets(&["frames"], &["frames"]).unwrap();
        check_targets(&[], &["frames"]).unwrap();
        let planned = check_targets(&["masks"], &["frames"])
            .unwrap_err()
            .to_string();
        assert!(
            planned.contains("not implemented yet (implemented: frames)"),
            "{planned}"
        );
        let typo = check_targets(&["frame"], &["frames"])
            .unwrap_err()
            .to_string();
        assert!(
            typo.contains("unknown stage frame") && typo.contains("masks"),
            "{typo}"
        );
    }

    #[test]
    fn missing_input_names_the_key_and_the_working_directory() {
        let err = check_input(Path::new("data/raw/nope.mp4"), Path::new("/somewhere"))
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("dataset.input data/raw/nope.mp4") && err.contains("/somewhere"),
            "{err}"
        );
        check_input(Path::new(env!("CARGO_MANIFEST_DIR")), Path::new("/")).unwrap();
    }
}
