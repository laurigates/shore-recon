use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use shoreline_core::{Config, Tools, dag};

/// Water-aware photogrammetry and Gaussian splatting pipeline.
#[derive(Debug, Parser)]
#[command(version)]
struct Cli {
    /// Host tools file; its tables replace those of tools.toml beside the config.
    #[arg(long, global = true, env = "SHORELINE_TOOLS")]
    tools: Option<PathBuf>,
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Run the pipeline for a dataset config.
    Run { config: PathBuf },
    /// Print the resolved config (dataset merged over defaults).
    Config { config: PathBuf },
    /// Print the stage execution order.
    Plan { config: PathBuf },
    /// Print the resolved tools (tools.toml beside the config, then --tools).
    Tools { config: PathBuf },
}

fn load(path: &Path) -> Result<Config> {
    Config::load(path).with_context(|| format!("loading {}", path.display()))
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
            let tools = Tools::load(&config, cli.tools.as_deref())
                .with_context(|| format!("loading tools for {}", config.display()))?;
            print!("{}", toml::to_string_pretty(&tools)?);
        }
        Command::Run { config } => {
            load(&config)?;
            bail!("pipeline execution is not implemented yet (M1)");
        }
    }
    Ok(())
}
