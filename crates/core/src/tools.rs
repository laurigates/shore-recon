//! External tool execution (the `ToolRunner`).
//!
//! How a tool runs is machine configuration, not dataset configuration
//! (ADR-0003). `tools.toml` beside the dataset config holds one table per
//! tool; an override file (`--tools` / `SHORELINE_TOOLS`) replaces whole tool
//! tables. Containers bind-mount each listed directory at the same path, so
//! tool arguments need no path translation.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Instant;

use serde::{Deserialize, Serialize};
use toml::{Table, Value};

use crate::config::read_table;
use crate::error::{Error, Result};

pub const TOOLS_FILE: &str = "tools.toml";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Runtime {
    Native,
    Podman,
    Docker,
}

/// How one tool runs on this machine.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ToolSpec {
    pub runtime: Runtime,
    /// Executable name or path for `native`; the command inside the image otherwise.
    pub program: String,
    /// OCI image pinned by digest (`name@sha256:…`); container runtimes only.
    pub image: Option<String>,
    /// Pass the host GPUs through; container runtimes only.
    #[serde(default)]
    pub gpu: bool,
}

impl ToolSpec {
    fn validate(&self, name: &str) -> Result<()> {
        let invalid = |msg: &str| Err(Error::Tools(format!("{name}: {msg}")));
        if self.program.is_empty() {
            return invalid("program must not be empty");
        }
        match (self.runtime, &self.image) {
            (Runtime::Native, Some(_)) => invalid("image is only valid for container runtimes"),
            (Runtime::Native, None) if self.gpu => {
                invalid("gpu is only valid for container runtimes")
            }
            (Runtime::Native, None) => Ok(()),
            (_, None) => invalid("container runtimes need an image"),
            (_, Some(image)) if !image.contains("@sha256:") => {
                invalid("image must be pinned by digest (name@sha256:...)")
            }
            (_, Some(_)) => Ok(()),
        }
    }
}

/// Tool specs keyed by tool name.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Tools(BTreeMap<String, ToolSpec>);

impl Tools {
    /// Load `tools.toml` beside `config` (optional), with `overlay` replacing
    /// whole tool tables.
    pub fn load(config: &Path, overlay: Option<&Path>) -> Result<Self> {
        let base = config.parent().unwrap_or(Path::new(".")).join(TOOLS_FILE);
        let mut table = if base.is_file() {
            read_table(&base)?
        } else {
            Table::new()
        };
        if let Some(overlay) = overlay {
            table.extend(read_table(overlay)?);
        }
        Self::from_table(table)
    }

    pub fn from_table(table: Table) -> Result<Self> {
        let tools: Self = Value::Table(table)
            .try_into()
            .map_err(|e: toml::de::Error| Error::Tools(e.message().to_owned()))?;
        for (name, spec) in &tools.0 {
            spec.validate(name)?;
        }
        Ok(tools)
    }

    pub fn get(&self, name: &str) -> Result<Tool> {
        let spec = self.0.get(name).ok_or_else(|| {
            Error::Tools(format!(
                "no [{name}] table in {TOOLS_FILE} beside the dataset config or in --tools"
            ))
        })?;
        Ok(Tool {
            name: name.to_owned(),
            spec: spec.clone(),
        })
    }
}

/// A tool by name. [`Tool::command`] re-validates the spec, as fields are public.
#[derive(Debug, Clone)]
pub struct Tool {
    pub name: String,
    pub spec: ToolSpec,
}

/// One call of a tool.
#[derive(Debug, Clone, Default)]
pub struct Invocation {
    pub args: Vec<OsString>,
    /// Absolute directories without `..` that the tool reads or writes.
    /// Containers bind-mount each at the same path; native runs ignore them.
    pub mounts: Vec<PathBuf>,
}

impl Invocation {
    pub fn new<I, S>(args: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<OsString>,
    {
        Self {
            args: args.into_iter().map(Into::into).collect(),
            mounts: Vec::new(),
        }
    }

    pub fn mount(mut self, dir: impl Into<PathBuf>) -> Self {
        self.mounts.push(dir.into());
        self
    }
}

/// What ran, for the stage manifest.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolRecord {
    pub tool: String,
    pub runtime: Runtime,
    pub image: Option<String>,
    pub argv: Vec<String>,
    pub seconds: f64,
    pub log: PathBuf,
}

impl Tool {
    /// The process to spawn for `inv`. Nothing is executed.
    pub fn command(&self, inv: &Invocation) -> Result<Command> {
        self.spec.validate(&self.name)?;
        let (engine, gpu_flag) = match self.spec.runtime {
            Runtime::Native => {
                let mut cmd = Command::new(&self.spec.program);
                cmd.args(&inv.args);
                return Ok(cmd);
            }
            Runtime::Podman => ("podman", ["--device", "nvidia.com/gpu=all"]),
            Runtime::Docker => ("docker", ["--gpus", "all"]),
        };
        let mut cmd = Command::new(engine);
        cmd.args(["run", "--rm"]);
        if self.spec.gpu {
            cmd.args(gpu_flag);
        }
        for dir in &inv.mounts {
            let parent = dir
                .components()
                .any(|c| c == std::path::Component::ParentDir);
            if !dir.is_absolute() || parent || dir.as_os_str().to_string_lossy().contains(':') {
                return Err(Error::Tools(format!(
                    "{}: mount {} must be an absolute path without '..' or ':'",
                    self.name,
                    dir.display()
                )));
            }
            let mut bind = dir.as_os_str().to_owned();
            bind.push(":");
            bind.push(dir);
            cmd.arg("-v").arg(bind);
        }
        let image = self.spec.image.as_deref().expect("validated above");
        cmd.arg(image).arg(&self.spec.program).args(&inv.args);
        Ok(cmd)
    }

    /// Run `inv` with stdin closed, writing the command line, stdout and
    /// stderr to `log`. A non-zero exit is an error that names the log.
    pub fn run(&self, inv: &Invocation, log: &Path) -> Result<ToolRecord> {
        let mut cmd = self.command(inv)?;
        let argv = argv(&cmd);
        let io = |path: &Path| {
            let path = path.to_owned();
            move |source| Error::Io { path, source }
        };
        if let Some(dir) = log.parent() {
            std::fs::create_dir_all(dir).map_err(io(dir))?;
        }
        let mut stdout = File::create(log).map_err(io(log))?;
        writeln!(stdout, "$ {}", argv.join(" ")).map_err(io(log))?;
        let stderr = stdout.try_clone().map_err(io(log))?;

        let start = Instant::now();
        let status = cmd
            .stdin(Stdio::null())
            .stdout(stdout)
            .stderr(stderr)
            .status()
            .map_err(|source| Error::Spawn {
                program: argv[0].clone(),
                source,
            })?;
        if !status.success() {
            return Err(Error::ToolFailed {
                tool: self.name.clone(),
                status,
                log: log.to_owned(),
            });
        }
        Ok(ToolRecord {
            tool: self.name.clone(),
            runtime: self.spec.runtime,
            image: self.spec.image.clone(),
            argv,
            seconds: start.elapsed().as_secs_f64(),
            log: log.to_owned(),
        })
    }

    /// Version string for fingerprints. Containers: the pinned image
    /// reference. Native: the first non-empty stdout line of `program probe…`.
    pub fn version(&self, probe: &[&str]) -> Result<String> {
        if let Some(image) = &self.spec.image {
            return Ok(image.clone());
        }
        let mut cmd = self.command(&Invocation::new(probe))?;
        let argv = argv(&cmd);
        let out = cmd
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output()
            .map_err(|source| Error::Spawn {
                program: argv[0].clone(),
                source,
            })?;
        let failed =
            |why: String| Error::Tools(format!("{}: `{}` {why}", self.name, argv.join(" ")));
        if !out.status.success() {
            return Err(failed(format!("exited with {}", out.status)));
        }
        String::from_utf8_lossy(&out.stdout)
            .lines()
            .map(str::trim)
            .find(|line| !line.is_empty())
            .map(str::to_owned)
            .ok_or_else(|| failed("printed no version".to_owned()))
    }
}

/// Program and arguments of `cmd`, lossily as strings.
pub fn argv(cmd: &Command) -> Vec<String> {
    std::iter::once(cmd.get_program())
        .chain(cmd.get_args())
        .map(|a| a.to_string_lossy().into_owned())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Error;

    const IMAGE: &str =
        "colmap/colmap@sha256:c1de51615dff33066cd305071226b6343c1168324ef3bd622f0825e55feb5efe";

    fn tools(s: &str) -> Result<Tools> {
        Tools::from_table(s.parse().unwrap())
    }

    fn native(program: &str) -> Tool {
        Tool {
            name: program.into(),
            spec: ToolSpec {
                runtime: Runtime::Native,
                program: program.into(),
                image: None,
                gpu: false,
            },
        }
    }

    fn container(runtime: Runtime, gpu: bool) -> Tool {
        Tool {
            name: "colmap".into(),
            spec: ToolSpec {
                runtime,
                program: "colmap".into(),
                image: Some(IMAGE.into()),
                gpu,
            },
        }
    }

    fn sh(script: &str) -> Invocation {
        Invocation::new(["-c", script])
    }

    fn tools_err(s: &str) -> String {
        match tools(s) {
            Err(Error::Tools(m)) => m,
            other => panic!("expected a tools error, got {other:?}"),
        }
    }

    #[test]
    fn native_runs_program_directly_and_ignores_mounts() {
        let inv = Invocation::new(["-i", "in.mp4"]).mount("/data");
        let cmd = native("ffmpeg").command(&inv).unwrap();
        assert_eq!(argv(&cmd), ["ffmpeg", "-i", "in.mp4"]);
    }

    #[test]
    fn docker_binds_mounts_at_the_same_path_and_passes_gpus() {
        let inv = Invocation::new(["help"]).mount("/work/a").mount("/data");
        let cmd = container(Runtime::Docker, true).command(&inv).unwrap();
        assert_eq!(
            argv(&cmd),
            [
                "docker",
                "run",
                "--rm",
                "--gpus",
                "all",
                "-v",
                "/work/a:/work/a",
                "-v",
                "/data:/data",
                IMAGE,
                "colmap",
                "help",
            ]
        );
    }

    #[test]
    fn podman_passes_gpus_as_a_cdi_device() {
        let cmd = container(Runtime::Podman, true)
            .command(&Invocation::new(["help"]))
            .unwrap();
        assert_eq!(
            argv(&cmd),
            [
                "podman",
                "run",
                "--rm",
                "--device",
                "nvidia.com/gpu=all",
                IMAGE,
                "colmap",
                "help",
            ]
        );
    }

    #[test]
    fn container_without_gpu_has_no_gpu_flag() {
        let cmd = container(Runtime::Docker, false)
            .command(&Invocation::default())
            .unwrap();
        assert_eq!(argv(&cmd), ["docker", "run", "--rm", IMAGE, "colmap"]);
    }

    #[test]
    fn mounts_must_be_absolute_and_colon_free() {
        let tool = container(Runtime::Docker, false);
        for bad in ["work/a", "/work/a:b", "/work/x/../a"] {
            let err = tool.command(&Invocation::default().mount(bad)).unwrap_err();
            assert!(matches!(&err, Error::Tools(m) if m.contains(bad)), "{err}");
        }
    }

    #[test]
    fn spec_validation() {
        let digest = format!("image = \"{IMAGE}\"");
        for (body, needle) in [
            ("runtime = \"docker\"\nprogram = \"x\"", "image"),
            (
                "runtime = \"docker\"\nprogram = \"x\"\nimage = \"x:latest\"",
                "digest",
            ),
            (
                &format!("runtime = \"native\"\nprogram = \"x\"\n{digest}"),
                "image",
            ),
            ("runtime = \"native\"\nprogram = \"x\"\ngpu = true", "gpu"),
            ("runtime = \"native\"\nprogram = \"\"", "program"),
            ("runtime = \"native\"\nprogram = \"x\"\nflags = 1", "flags"),
            ("runtime = \"apptainer\"\nprogram = \"x\"", "apptainer"),
        ] {
            let err = tools_err(&format!("[x]\n{body}\n"));
            assert!(err.contains(needle), "{body:?}: {err}");
        }
        tools(&format!(
            "[x]\nruntime = \"podman\"\nprogram = \"x\"\ngpu = true\n{digest}\n"
        ))
        .unwrap();
    }

    #[test]
    fn get_names_the_missing_tool() {
        let t = tools("[ffmpeg]\nruntime = \"native\"\nprogram = \"ffmpeg\"\n").unwrap();
        assert_eq!(t.get("ffmpeg").unwrap().spec.program, "ffmpeg");
        assert!(matches!(t.get("colmap"), Err(Error::Tools(m)) if m.contains("colmap")));
    }

    #[test]
    fn overlay_replaces_whole_tool_tables() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join(TOOLS_FILE),
            format!(
                "[ffmpeg]\nruntime = \"native\"\nprogram = \"ffmpeg\"\n\
                 [colmap]\nruntime = \"docker\"\nprogram = \"colmap\"\ngpu = true\nimage = \"{IMAGE}\"\n"
            ),
        )
        .unwrap();
        let overlay = dir.path().join("host.toml");
        std::fs::write(
            &overlay,
            "[colmap]\nruntime = \"native\"\nprogram = \"/opt/colmap/bin/colmap\"\n",
        )
        .unwrap();
        let config = dir.path().join("dataset.toml");

        let t = Tools::load(&config, Some(&overlay)).unwrap();
        assert_eq!(
            t.get("colmap").unwrap().spec,
            ToolSpec {
                runtime: Runtime::Native,
                program: "/opt/colmap/bin/colmap".into(),
                image: None,
                gpu: false,
            }
        );
        assert_eq!(t.get("ffmpeg").unwrap().spec.runtime, Runtime::Native);
        assert_eq!(
            Tools::load(&config, None)
                .unwrap()
                .get("colmap")
                .unwrap()
                .spec
                .runtime,
            Runtime::Docker
        );
    }

    #[test]
    fn missing_tools_file_is_empty_but_missing_overlay_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let config = dir.path().join("dataset.toml");
        assert!(Tools::load(&config, None).unwrap().get("ffmpeg").is_err());
        let missing = dir.path().join("nope.toml");
        assert!(matches!(
            Tools::load(&config, Some(&missing)),
            Err(Error::Io { path, .. }) if path == missing
        ));
    }

    #[test]
    fn repo_tools_config_loads() {
        let config =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../configs/reposalmi-laajasalo.toml");
        let t = Tools::load(&config, None).unwrap();
        assert_eq!(t.get("ffmpeg").unwrap().spec.runtime, Runtime::Native);
        let segment = t.get("segment").unwrap().spec;
        assert_eq!(segment.runtime, Runtime::Docker);
        let image = segment.image.unwrap();
        assert!(
            image.starts_with("ghcr.io/laurigates/shoreline-segmentation@sha256:"),
            "{image}"
        );
    }

    #[test]
    fn run_logs_command_line_stdout_and_stderr() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("logs/x.log");
        let rec = native("sh")
            .run(&sh("echo out; echo err >&2"), &log)
            .unwrap();
        let text = std::fs::read_to_string(&log).unwrap();
        assert!(
            text.starts_with("$ sh -c echo out; echo err >&2\n"),
            "{text}"
        );
        assert!(text.contains("out\n") && text.contains("err\n"), "{text}");
        assert_eq!(rec.tool, "sh");
        assert_eq!(rec.runtime, Runtime::Native);
        assert_eq!(rec.argv, ["sh", "-c", "echo out; echo err >&2"]);
        assert_eq!(rec.log, log);
    }

    #[test]
    fn run_does_not_wait_on_stdin() {
        let dir = tempfile::tempdir().unwrap();
        native("sh")
            .run(&sh("cat"), &dir.path().join("cat.log"))
            .unwrap();
    }

    #[test]
    fn nonzero_exit_is_an_error_naming_the_log() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("fail.log");
        let err = native("sh").run(&sh("exit 3"), &log).unwrap_err();
        assert!(
            matches!(&err, Error::ToolFailed { tool, status, log: l }
                if tool == "sh" && status.code() == Some(3) && *l == log),
            "{err}"
        );
    }

    #[test]
    fn missing_program_is_a_spawn_error() {
        let dir = tempfile::tempdir().unwrap();
        let err = native("shoreline-no-such-tool")
            .run(&Invocation::default(), &dir.path().join("x.log"))
            .unwrap_err();
        assert!(
            matches!(&err, Error::Spawn { program, .. } if program == "shoreline-no-such-tool"),
            "{err}"
        );
    }

    #[test]
    fn native_version_is_the_first_nonempty_stdout_line() {
        let v = native("sh")
            .version(&["-c", "printf '\\n  v1.2 build\\nmore\\n'; echo noise >&2"])
            .unwrap();
        assert_eq!(v, "v1.2 build");
    }

    #[test]
    fn version_probe_failure_or_silence_is_an_error() {
        assert!(native("sh").version(&["-c", "exit 1"]).is_err());
        assert!(native("sh").version(&["-c", "true"]).is_err());
    }

    #[test]
    fn container_version_is_the_image_reference_without_running() {
        let v = container(Runtime::Docker, false)
            .version(&["--version"])
            .unwrap();
        assert_eq!(v, IMAGE);
    }
}
