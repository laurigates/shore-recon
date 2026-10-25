use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use shoreline_core::pipeline::{self, Event, StageResult};
use shoreline_core::{Config, RunContext, Stage, Tools, dag};
use shoreline_frames::Frames;

/// Water-aware photogrammetry and Gaussian splatting pipeline.
#[derive(Debug, Parser)]
#[command(version)]
struct Cli {
    /// Host tools file; its tables replace those of tools.toml beside the config.
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
    /// Print the resolved config (dataset merged over defaults).
    Config { config: PathBuf },
    /// Print the stage execution order.
    Plan { config: PathBuf },
    /// Print the resolved tools (tools.toml beside the config, then --tools).
    Tools { config: PathBuf },
}

/// Stages implemented so far; `run` refuses targets outside this list.
const STAGES: &[&dyn Stage] = &[&Frames];

fn load(path: &Path) -> Result<Config> {
    Config::load(path).with_context(|| format!("loading {}", path.display()))
}

fn load_tools(config: &Path, overlay: Option<&Path>) -> Result<Tools> {
    Tools::load(config, overlay).with_context(|| format!("loading tools for {}", config.display()))
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
            print!("{}", toml::to_string_pretty(&load(&config)?)?);
        }
        Command::Plan { config } => {
            load(&config)?;
            for (i, stage) in dag::topo_order(dag::PIPELINE)?.iter().enumerate() {
                println!("{} {stage}", i + 1);
            }
        }
        Command::Tools { config } => {
            let tools = load_tools(&config, cli.tools.as_deref())?;
            print!("{}", toml::to_string_pretty(&tools)?);
        }
        Command::Run { config, stages } => {
            let tools = load_tools(&config, cli.tools.as_deref())?;
            let ctx = RunContext::new(load(&config)?, tools, &cli.work)?;
            let targets: Vec<&str> = stages.iter().map(String::as_str).collect();
            for result in pipeline::run(&ctx, STAGES, &targets, &mut report)? {
                println!("{}", summary(&result));
            }
        }
    }
    Ok(())
}
