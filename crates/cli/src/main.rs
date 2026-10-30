mod progress;

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use clap::{Args, CommandFactory, Parser, Subcommand};
use clap_complete::Shell;
use shoreline_core::pipeline::{self, StageResult};
use shoreline_core::{Config, RunContext, Stage, Tools, dag, dirs};
use shoreline_dense::Dense;
use shoreline_export::{Export, copy_out};
use shoreline_frames::Frames;
use shoreline_masks::Masks;
use shoreline_post::Post;
use shoreline_sfm::Sfm;
use shoreline_splat::Splat;
use shoreline_video::VideoStage;

use progress::{Reporter, SyncStage};

/// Water-aware photogrammetry and Gaussian splatting pipeline.
///
/// `shoreline-recon <video>` reconstructs one clip into ./<name>-recon/:
/// the mesh with its water plane, the splat and a report.
#[derive(Debug, Parser)]
#[command(version, arg_required_else_help = true)]
struct Cli {
    /// Host tools file; its tables replace those of every other tools layer.
    #[arg(long, global = true, env = "SHORELINE_TOOLS")]
    tools: Option<PathBuf>,
    /// Root of the stage cache: outputs go to <work>/<dataset>/<stage>/<fingerprint>/.
    /// Default: ./work for subcommands, the user cache directory for a video.
    #[arg(long, global = true, env = "SHORELINE_WORK")]
    work: Option<PathBuf>,
    #[command(flatten)]
    quick: QuickArgs,
    #[command(subcommand)]
    command: Option<Command>,
}

/// Reconstruct one video without a dataset config (ADR-0017).
#[derive(Debug, Args)]
struct QuickArgs {
    /// Video to reconstruct.
    video: Option<PathBuf>,
    /// Output directory for the deliverables. Default: ./<name>-recon.
    #[arg(short, long)]
    out: Option<PathBuf>,
    /// Dataset name (cache directory and report). Default: the video's file stem.
    #[arg(long)]
    name: Option<String>,
    /// Frames extracted per second of video (frames.fps).
    #[arg(long)]
    fps: Option<f64>,
    /// Splat training steps (splat.steps).
    #[arg(long)]
    steps: Option<u32>,
    /// Feature matcher (sfm.matcher).
    #[arg(long, value_parser = ["sequential", "exhaustive"])]
    matcher: Option<String>,
    /// Also render the showcase and processing-step videos (export.videos).
    #[arg(long)]
    videos: bool,
    /// Partial config (any keys of defaults.toml), merged over the defaults.
    #[arg(long)]
    config: Option<PathBuf>,
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
    /// Print the completion script for a shell, e.g.
    /// `eval "$(shoreline-recon completions zsh)"` in ~/.zshrc.
    Completions { shell: Shell },
}

/// The binary name the completion scripts complete.
const BIN: &str = "shoreline-recon";

/// The completion script for `shell`.
fn completions(shell: Shell) -> Vec<u8> {
    let mut script = Vec::new();
    clap_complete::generate(shell, &mut Cli::command(), BIN, &mut script);
    script
}

/// The stages for `config`, in pipeline order; `run` refuses targets outside
/// this list. Export depends on the video stage only with `export.videos`.
/// `Sync` so the progress watcher can read logs with them.
fn stages(config: &Config) -> Vec<SyncStage> {
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

/// `stages` as the pipeline takes them.
fn as_stages(stages: &[SyncStage]) -> Vec<&dyn Stage> {
    stages.iter().map(|&s| s as &dyn Stage).collect()
}

/// Run `targets` with progress on stderr and a summary table at the end.
fn run_reported(
    ctx: &RunContext,
    stages: &[SyncStage],
    targets: &[&str],
) -> Result<Vec<StageResult>> {
    let all = as_stages(stages);
    let plan = pipeline::plan(&all, targets)?;
    let work_root = ctx.work_dir.parent().unwrap_or(&ctx.work_dir);
    let mut reporter = Reporter::new(stages, &plan, &ctx.config, work_root);
    let results = pipeline::run(ctx, &all, targets, &mut |e| reporter.event(e))?;
    reporter.summary();
    Ok(results)
}

/// What `run` builds when no `--stage` is given: the deliverables.
const DEFAULT_TARGETS: &[&str] = &["export"];

impl QuickArgs {
    fn is_set(&self) -> bool {
        self.video.is_some()
            || self.out.is_some()
            || self.name.is_some()
            || self.fps.is_some()
            || self.steps.is_some()
            || self.matcher.is_some()
            || self.videos
            || self.config.is_some()
    }
}

/// A dataset name from a file stem: a single path component made of letters,
/// digits, '.', '_' and '-', starting with a letter or digit.
fn dataset_name(stem: &str) -> String {
    let name: String = stem
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
                c
            } else {
                '-'
            }
        })
        .collect();
    let name = name.trim_start_matches(|c: char| !c.is_ascii_alphanumeric());
    if name.is_empty() {
        "capture".to_owned()
    } else {
        name.to_owned()
    }
}

/// Command-line settings as a config layer over every file.
fn overrides(q: &QuickArgs, name: &str, input: &Path) -> Result<toml::Table> {
    let mut t = toml::Table::new();
    let mut section = |key: &str, field: &str, value: toml::Value| {
        t.entry(key)
            .or_insert_with(|| toml::Value::Table(toml::Table::new()))
            .as_table_mut()
            .expect("sections are tables")
            .insert(field.to_owned(), value);
    };
    section("dataset", "name", name.into());
    let input = input
        .to_str()
        .with_context(|| format!("{} is not valid UTF-8", input.display()))?;
    section("dataset", "input", input.into());
    if let Some(fps) = q.fps {
        section("frames", "fps", fps.into());
    }
    if let Some(steps) = q.steps {
        section("splat", "steps", i64::from(steps).into());
    }
    if let Some(matcher) = &q.matcher {
        section("sfm", "matcher", matcher.as_str().into());
    }
    if q.videos {
        section("export", "videos", true.into());
    }
    Ok(t)
}

/// `shoreline-recon <video>`: run to export, then copy the deliverables out.
fn quick(q: &QuickArgs, video: &Path, tools: Option<&Path>, work: Option<&Path>) -> Result<()> {
    let input = std::fs::canonicalize(video)
        .with_context(|| format!("video {} not found", video.display()))?;
    let stem = input.file_stem().unwrap_or_default().to_string_lossy();
    let name = q.name.clone().unwrap_or_else(|| dataset_name(&stem));
    let user = dirs::config_dir();
    let files: Vec<&Path> = q.config.as_deref().into_iter().collect();
    let config = Config::layered(user.as_deref(), &files, overrides(q, &name, &input)?)
        .context("resolving the config")?;
    let tools =
        Tools::layered(user.as_deref(), q.config.as_deref(), tools).context("loading tools")?;
    if config.export.videos && tools.get("render").is_err() {
        bail!(
            "--videos needs a [render] tool, and none is configured: add one to the user's \
             tools.toml or pass --tools (the CUDA render image runs on a remote host)"
        );
    }
    let work = match work {
        Some(w) => w.to_path_buf(),
        None => dirs::cache_dir().unwrap_or_else(|| PathBuf::from("work")),
    };
    let videos = config.export.videos;
    let ctx = RunContext::new(config, tools, &work)?;
    let results = run_reported(&ctx, &stages(&ctx.config), DEFAULT_TARGETS)?;
    let export = results
        .iter()
        .find(|r| r.stage == shoreline_export::NAME)
        .context("the pipeline did not run export")?;
    let out = q
        .out
        .clone()
        .unwrap_or_else(|| PathBuf::from(format!("{name}-recon")));
    for file in copy_out(&export.dir, &out, &ctx.config, videos)? {
        println!("{}", file.display());
    }
    Ok(())
}

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
    let Some(command) = cli.command else {
        let video = cli.quick.video.as_deref().context("no video given")?;
        return quick(&cli.quick, video, cli.tools.as_deref(), cli.work.as_deref());
    };
    if cli.quick.is_set() {
        bail!("a video and its options cannot be combined with a subcommand");
    }
    let work = cli.work.unwrap_or_else(|| PathBuf::from("work"));
    match command {
        Command::Config { config } => {
            print!("{}", toml::to_string_pretty(&load(config.as_deref())?)?);
        }
        Command::Plan { config } => {
            let config = load(config.as_deref())?;
            for (i, stage) in pipeline::plan(&as_stages(&stages(&config)), DEFAULT_TARGETS)?
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
        Command::Completions { shell } => {
            std::io::Write::write_all(&mut std::io::stdout(), &completions(shell))?;
        }
        Command::Run { config, stages } => {
            let tools = load_tools(Some(&config), cli.tools.as_deref())?;
            let ctx = RunContext::new(load(Some(&config))?, tools, &work)?;
            let mut targets: Vec<&str> = stages.iter().map(String::as_str).collect();
            let all = self::stages(&ctx.config);
            let implemented: Vec<&str> = all.iter().map(|s| s.name()).collect();
            check_targets(&targets, &implemented)?;
            if targets.is_empty() {
                targets = DEFAULT_TARGETS.to_vec();
            }
            check_input(&ctx.config.dataset.input, &std::env::current_dir()?)?;
            for result in run_reported(&ctx, &all, &targets)? {
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
            pipeline::plan(&as_stages(&stages(&config)), DEFAULT_TARGETS).unwrap(),
            ["frames", "masks", "sfm", "dense", "splat", "post", "export"]
        );

        config.export.videos = true;
        expected.last_mut().unwrap().1.push("video");
        assert_eq!(graph(&config), expected);
        assert_eq!(
            pipeline::plan(&as_stages(&stages(&config)), DEFAULT_TARGETS).unwrap(),
            [
                "frames", "masks", "sfm", "dense", "splat", "post", "video", "export"
            ]
        );
    }

    #[test]
    fn cli_definition_is_consistent() {
        Cli::command().debug_assert();
    }

    /// The scripts in completions/ ship in the release archives (dist `include`);
    /// regenerate them with `just completions` after changing the CLI.
    #[test]
    fn shipped_completions_match_the_cli() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("completions");
        for (shell, file) in [
            (Shell::Bash, "shoreline-recon.bash"),
            (Shell::Zsh, "_shoreline-recon"),
            (Shell::Fish, "shoreline-recon.fish"),
        ] {
            let shipped = std::fs::read(dir.join(file)).unwrap_or_default();
            assert!(
                shipped == completions(shell),
                "completions/{file} is out of date with the CLI: run `just completions`"
            );
        }
    }

    #[test]
    fn completions_take_a_shell() {
        Cli::try_parse_from(["shoreline-recon", "completions", "zsh"]).unwrap();
        assert!(Cli::try_parse_from(["shoreline-recon", "completions"]).is_err());
        assert!(Cli::try_parse_from(["shoreline-recon", "completions", "cmd"]).is_err());
    }

    #[test]
    fn a_video_or_a_subcommand_but_not_both() {
        let c = Cli::try_parse_from(["shoreline-recon", "clip.mp4"]).unwrap();
        assert_eq!(c.quick.video.as_deref(), Some(Path::new("clip.mp4")));
        assert!(c.command.is_none());
        let c = Cli::try_parse_from([
            "shoreline-recon",
            "clip.mp4",
            "-o",
            "out",
            "--name",
            "pier",
            "--fps",
            "3",
            "--steps",
            "7000",
            "--matcher",
            "exhaustive",
            "--videos",
            "--config",
            "my.toml",
            "--tools",
            "host.toml",
            "--work",
            "w",
        ])
        .unwrap();
        assert!(c.quick.videos && c.quick.steps == Some(7000));
        assert_eq!(c.work.as_deref(), Some(Path::new("w")));
        let c =
            Cli::try_parse_from(["shoreline-recon", "--tools", "h.toml", "run", "c.toml"]).unwrap();
        assert!(matches!(c.command, Some(Command::Run { .. })) && c.tools.is_some());
        assert!(!c.quick.is_set());
        let c = Cli::try_parse_from(["shoreline-recon", "--fps", "3", "plan"]).unwrap();
        assert!(c.quick.is_set() && c.command.is_some()); // refused in main
        assert!(Cli::try_parse_from(["shoreline-recon", "clip.mp4", "--matcher", "x"]).is_err());
        assert!(Cli::try_parse_from(["shoreline-recon"]).is_err());
    }

    #[test]
    fn dataset_names_are_plain_path_components() {
        assert_eq!(dataset_name("DJI_0849"), "DJI_0849");
        assert_eq!(dataset_name("my pier (2)"), "my-pier--2-");
        assert_eq!(dataset_name("..hidden"), "hidden");
        assert_eq!(dataset_name("äö"), "capture");
    }

    #[test]
    fn overrides_become_the_top_config_layer() {
        let c = Cli::try_parse_from([
            "shoreline-recon",
            "clip.mp4",
            "--fps",
            "3",
            "--steps",
            "7000",
            "--matcher",
            "exhaustive",
            "--videos",
        ])
        .unwrap();
        let t = overrides(&c.quick, "pier", Path::new("/data/clip.mp4")).unwrap();
        let cfg = Config::layered(None, &[], t).unwrap();
        assert_eq!(cfg.dataset.name, "pier");
        assert_eq!(cfg.dataset.input, Path::new("/data/clip.mp4"));
        assert_eq!(cfg.frames.fps, 3.0);
        assert_eq!(cfg.splat.steps, 7000);
        assert_eq!(cfg.sfm.matcher, shoreline_core::config::Matcher::Exhaustive);
        assert!(cfg.export.videos);
        let plain = Cli::try_parse_from(["shoreline-recon", "clip.mp4"]).unwrap();
        let cfg = Config::layered(
            None,
            &[],
            overrides(&plain.quick, "pier", Path::new("/c.mp4")).unwrap(),
        )
        .unwrap();
        assert!(!cfg.export.videos);
        assert_eq!(cfg.splat.steps, 30000);
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
