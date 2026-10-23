use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use shoreline_core::{Config, dag};

/// Water-aware photogrammetry and Gaussian splatting pipeline.
#[derive(Debug, Parser)]
#[command(version)]
struct Cli {
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
}

fn load(path: &Path) -> Result<Config> {
    Config::load(path).with_context(|| format!("loading {}", path.display()))
}

fn main() -> Result<()> {
    match Cli::parse().command {
        Command::Config { config } => {
            print!("{}", toml::to_string_pretty(&load(&config)?)?);
        }
        Command::Plan { config } => {
            load(&config)?;
            for (i, stage) in dag::topo_order(dag::PIPELINE)?.iter().enumerate() {
                println!("{} {stage}", i + 1);
            }
        }
        Command::Run { config } => {
            load(&config)?;
            bail!("pipeline execution is not implemented yet (M1)");
        }
    }
    Ok(())
}
