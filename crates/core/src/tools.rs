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
    /// `apptainer exec` of the OCI image (`docker://…`); Apptainer caches the
    /// converted SIF and runs as the invoking user.
    Apptainer,
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
    /// Run the container on another host over ssh (ADR-0009); container runtimes only.
    pub remote: Option<Remote>,
}

/// Where a remote container runs. Each mount `/p` is mirrored to `<root>/p`
/// there and bound at `/p`, so tool arguments stay host paths (ADR-0009).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Remote {
    /// ssh destination, as for `ssh <host>`.
    pub host: String,
    /// Absolute directory on the remote host that holds the mirrored mounts.
    pub root: PathBuf,
    /// Environment for the remote command, such as `APPTAINER_CACHEDIR`.
    #[serde(default)]
    pub env: BTreeMap<String, String>,
}

/// rsync passes remote paths to the remote shell unquoted, so remote paths are
/// limited to characters no shell treats specially.
fn shell_safe(path: &Path) -> bool {
    path.to_str().is_some_and(|s| {
        s.chars()
            .all(|c| c.is_ascii_alphanumeric() || "/._-+@".contains(c))
    })
}

/// `s` as one single-quoted word, for POSIX shells and zsh.
fn sh_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// `ssh -o BatchMode=yes <host> -- <words, each quoted>`.
fn ssh(host: &str, words: impl IntoIterator<Item = String>) -> Command {
    let line = words
        .into_iter()
        .map(|w| sh_quote(&w))
        .collect::<Vec<_>>()
        .join(" ");
    let mut cmd = Command::new("ssh");
    cmd.args(["-o", "BatchMode=yes", host, "--"]).arg(line);
    cmd
}

/// The rsync calls around one remote run (ADR-0009, decision 2).
#[derive(Debug)]
pub struct Sync {
    /// Creates the mirror directories on the remote host.
    pub prepare: Command,
    /// Mirrors each mount to the remote host, deleting stale remote files.
    pub push: Vec<Command>,
    /// Copies each mirrored mount back.
    pub pull: Vec<Command>,
}

impl Remote {
    fn validate(&self) -> std::result::Result<(), &'static str> {
        if self.host.is_empty() || self.host.starts_with('-') {
            return Err("remote host must be an ssh destination");
        }
        // `--delete` pushes into `<root>/p`, so `/` would mirror onto the host's own `/p`.
        if !self.root.is_absolute() || !shell_safe(&self.root) || self.root.parent().is_none() {
            return Err("remote root must be an absolute path of [A-Za-z0-9/._-+@], not /");
        }
        let key_ok = |k: &str| {
            k.chars().next().is_some_and(|c| !c.is_ascii_digit())
                && k.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        };
        if !self.env.keys().all(|k| key_ok(k)) {
            return Err("remote env names must be [A-Za-z_][A-Za-z0-9_]*");
        }
        Ok(())
    }

    /// `/p` mirrored under the remote root.
    fn mirror(&self, dir: &Path) -> PathBuf {
        self.root
            .join(dir.strip_prefix("/").expect("mounts are absolute"))
    }
}

impl ToolSpec {
    fn validate(&self, name: &str) -> Result<()> {
        let invalid = |msg: &str| Err(Error::Tools(format!("{name}: {msg}")));
        if self.program.is_empty() {
            return invalid("program must not be empty");
        }
        if let Some(remote) = &self.remote {
            if self.runtime == Runtime::Native {
                return invalid("remote is only valid for container runtimes");
            }
            if let Err(msg) = remote.validate() {
                return invalid(msg);
            }
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
    /// ssh destination for remote runs (ADR-0009).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
    pub argv: Vec<String>,
    pub seconds: f64,
    pub log: PathBuf,
}

impl Tool {
    /// The process to spawn for `inv`. Nothing is executed. A remote tool's
    /// process is the ssh call; see [`Tool::sync`] for its file transfers.
    pub fn command(&self, inv: &Invocation) -> Result<Command> {
        self.spec.validate(&self.name)?;
        let (engine, prefix, gpu_flag, bind_flag): (_, &[&str], &[&str], _) =
            match self.spec.runtime {
                Runtime::Native => {
                    let mut cmd = Command::new(&self.spec.program);
                    cmd.args(&inv.args);
                    return Ok(cmd);
                }
                Runtime::Podman => (
                    "podman",
                    &["run", "--rm"],
                    &["--device", "nvidia.com/gpu=all"],
                    "-v",
                ),
                Runtime::Docker => ("docker", &["run", "--rm"], &["--gpus", "all"], "-v"),
                Runtime::Apptainer => ("apptainer", &["exec"], &["--nv"], "--bind"),
            };
        self.check_mounts(inv)?;
        let remote = self.spec.remote.as_ref();
        let mut words: Vec<OsString> = vec![engine.into()];
        words.extend(prefix.iter().map(OsString::from));
        if self.spec.gpu {
            words.extend(gpu_flag.iter().map(OsString::from));
        }
        for dir in &inv.mounts {
            let mut bind = match remote {
                Some(remote) => remote.mirror(dir).into_os_string(),
                None => dir.as_os_str().to_owned(),
            };
            bind.push(":");
            bind.push(dir);
            words.push(bind_flag.into());
            words.push(bind);
        }
        let image = self.spec.image.as_deref().expect("validated above");
        words.push(match self.spec.runtime {
            Runtime::Apptainer => format!("docker://{image}").into(),
            _ => image.into(),
        });
        words.push(self.spec.program.clone().into());
        words.extend(inv.args.iter().cloned());

        let Some(remote) = remote else {
            let mut cmd = Command::new(&words[0]);
            cmd.args(&words[1..]);
            return Ok(cmd);
        };
        let env = (!remote.env.is_empty()).then(|| "env".to_owned());
        let vars = remote.env.iter().map(|(k, v)| format!("{k}={v}"));
        let words = words.iter().map(|w| w.to_string_lossy().into_owned());
        Ok(ssh(&remote.host, env.into_iter().chain(vars).chain(words)))
    }

    /// The file transfers around a remote run of `inv` that logs to `log`;
    /// `None` for local tools. A log inside a mount is excluded both ways: it
    /// is written locally during the run, and pulling the host's copy back
    /// would overwrite it.
    pub fn sync(&self, inv: &Invocation, log: &Path) -> Result<Option<Sync>> {
        self.spec.validate(&self.name)?;
        let Some(remote) = &self.spec.remote else {
            return Ok(None);
        };
        self.check_mounts(inv)?;
        let slash = |p: &Path| {
            let mut s = p.as_os_str().to_owned();
            s.push("/");
            s
        };
        let at_host = |p: &Path| {
            let mut s = OsString::from(format!("{}:", remote.host));
            s.push(slash(p));
            s
        };
        let rsync = |args: &[&str], dir: &Path, from: OsString, to: OsString| {
            let mut cmd = Command::new("rsync");
            cmd.args(args);
            if let Ok(rel) = log.strip_prefix(dir) {
                let mut exclude = OsString::from("--exclude=/");
                exclude.push(rel);
                cmd.arg(exclude);
            }
            cmd.arg(from).arg(to);
            cmd
        };
        let mirrors = inv.mounts.iter().map(|d| (d, remote.mirror(d)));
        let mkdir = ["mkdir", "-p"].map(String::from).into_iter().chain(
            mirrors
                .clone()
                .map(|(_, m)| m.to_string_lossy().into_owned()),
        );
        Ok(Some(Sync {
            prepare: ssh(&remote.host, mkdir),
            push: mirrors
                .clone()
                .map(|(d, m)| rsync(&["-a", "--delete"], d, slash(d), at_host(&m)))
                .collect(),
            pull: mirrors
                .map(|(d, m)| rsync(&["-a"], d, at_host(&m), slash(d)))
                .collect(),
        }))
    }

    /// Mounts must be absolute, free of `..`, and expressible in the bind flag;
    /// remote mounts must also be shell-safe.
    fn check_mounts(&self, inv: &Invocation) -> Result<()> {
        // `--bind` takes a comma-separated list, so a comma would split the path.
        let forbidden: &[char] = match self.spec.runtime {
            Runtime::Apptainer => &[':', ','],
            _ => &[':'],
        };
        let remote = self.spec.remote.is_some();
        for dir in &inv.mounts {
            let parent = dir
                .components()
                .any(|c| c == std::path::Component::ParentDir);
            if !dir.is_absolute() || parent || dir.as_os_str().to_string_lossy().contains(forbidden)
            {
                return Err(Error::Tools(format!(
                    "{}: mount {} must be an absolute path without '..' or {}",
                    self.name,
                    dir.display(),
                    forbidden
                        .iter()
                        .map(|c| format!("'{c}'"))
                        .collect::<Vec<_>>()
                        .join(" or ")
                )));
            }
            if remote && !shell_safe(dir) {
                return Err(Error::Tools(format!(
                    "{}: remote mount {} must use only [A-Za-z0-9/._-+@]",
                    self.name,
                    dir.display()
                )));
            }
        }
        Ok(())
    }

    /// Run `inv` with stdin closed, writing the command line, stdout and
    /// stderr to `log`. A remote run also logs its transfers: push, run, pull.
    /// A non-zero exit of any step is an error that names the log.
    pub fn run(&self, inv: &Invocation, log: &Path) -> Result<ToolRecord> {
        let mut cmd = self.command(inv)?;
        let sync = self.sync(inv, log)?;
        let argv = argv(&cmd);
        let io = |path: &Path| {
            let path = path.to_owned();
            move |source| Error::Io { path, source }
        };
        if let Some(dir) = log.parent() {
            std::fs::create_dir_all(dir).map_err(io(dir))?;
        }
        let file = File::create(log).map_err(io(log))?;
        let step = |cmd: &mut Command| -> Result<()> {
            let argv = self::argv(cmd);
            let mut stdout = file.try_clone().map_err(io(log))?;
            writeln!(stdout, "$ {}", argv.join(" ")).map_err(io(log))?;
            let stderr = stdout.try_clone().map_err(io(log))?;
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
            Ok(())
        };

        let start = Instant::now();
        match sync {
            None => step(&mut cmd)?,
            Some(mut sync) => {
                step(&mut sync.prepare)?;
                sync.push.iter_mut().try_for_each(&step)?;
                step(&mut cmd)?;
                sync.pull.iter_mut().try_for_each(&step)?;
            }
        }
        Ok(ToolRecord {
            tool: self.name.clone(),
            runtime: self.spec.runtime,
            image: self.spec.image.clone(),
            host: self.spec.remote.as_ref().map(|r| r.host.clone()),
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
                remote: None,
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
                remote: None,
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
    fn apptainer_execs_the_oci_image_with_binds_and_nv() {
        let inv = Invocation::new(["help"]).mount("/work/a").mount("/data");
        let cmd = container(Runtime::Apptainer, true).command(&inv).unwrap();
        let docker_ref = format!("docker://{IMAGE}");
        assert_eq!(
            argv(&cmd),
            [
                "apptainer",
                "exec",
                "--nv",
                "--bind",
                "/work/a:/work/a",
                "--bind",
                "/data:/data",
                &docker_ref,
                "colmap",
                "help",
            ]
        );
    }

    #[test]
    fn apptainer_without_gpu_has_no_nv_flag() {
        let cmd = container(Runtime::Apptainer, false)
            .command(&Invocation::default())
            .unwrap();
        let docker_ref = format!("docker://{IMAGE}");
        assert_eq!(argv(&cmd), ["apptainer", "exec", &docker_ref, "colmap"]);
    }

    #[test]
    fn apptainer_mounts_must_be_comma_free() {
        // `--bind` splits its value on commas.
        let err = container(Runtime::Apptainer, false)
            .command(&Invocation::default().mount("/work/a,b"))
            .unwrap_err();
        assert!(
            matches!(&err, Error::Tools(m) if m.contains("/work/a,b")),
            "{err}"
        );
        container(Runtime::Docker, false)
            .command(&Invocation::default().mount("/work/a,b"))
            .unwrap();
    }

    fn remote(mut tool: Tool) -> Tool {
        tool.spec.remote = Some(Remote {
            host: "gpu".into(),
            root: "/srv/mirror".into(),
            env: BTreeMap::from([("APPTAINER_CACHEDIR".into(), "/srv/cache".into())]),
        });
        tool
    }

    #[test]
    fn remote_runs_the_container_over_ssh_with_mirrored_binds() {
        let inv = Invocation::new(["help", "it's"]).mount("/work/a");
        let cmd = remote(container(Runtime::Apptainer, true))
            .command(&inv)
            .unwrap();
        let remote_cmd = format!(
            "'env' 'APPTAINER_CACHEDIR=/srv/cache' 'apptainer' 'exec' '--nv' \
             '--bind' '/srv/mirror/work/a:/work/a' 'docker://{IMAGE}' 'colmap' 'help' 'it'\\''s'"
        );
        assert_eq!(
            argv(&cmd),
            ["ssh", "-o", "BatchMode=yes", "gpu", "--", &remote_cmd]
        );
    }

    #[test]
    fn remote_without_env_has_no_env_prefix() {
        let mut tool = remote(container(Runtime::Docker, false));
        tool.spec.remote.as_mut().unwrap().env.clear();
        let cmd = tool.command(&Invocation::new(["help"])).unwrap();
        assert_eq!(
            argv(&cmd).last().unwrap(),
            &format!("'docker' 'run' '--rm' '{IMAGE}' 'colmap' 'help'")
        );
    }

    #[test]
    fn remote_sync_mirrors_each_mount_both_ways_except_the_log() {
        // The log is written locally while the tool runs, so syncing it would
        // overwrite it with the host's stale copy.
        let inv = Invocation::new(["help"]).mount("/work/a").mount("/data");
        let log = Path::new("/work/a/logs/x.log");
        let sync = remote(container(Runtime::Apptainer, false))
            .sync(&inv, log)
            .unwrap()
            .unwrap();
        assert_eq!(
            argv(&sync.prepare),
            [
                "ssh",
                "-o",
                "BatchMode=yes",
                "gpu",
                "--",
                "'mkdir' '-p' '/srv/mirror/work/a' '/srv/mirror/data'"
            ]
        );
        let argvs = |cmds: &[Command]| cmds.iter().map(argv).collect::<Vec<_>>();
        assert_eq!(
            argvs(&sync.push),
            [
                vec![
                    "rsync",
                    "-a",
                    "--delete",
                    "--exclude=/logs/x.log",
                    "/work/a/",
                    "gpu:/srv/mirror/work/a/"
                ],
                vec!["rsync", "-a", "--delete", "/data/", "gpu:/srv/mirror/data/"],
            ]
        );
        assert_eq!(
            argvs(&sync.pull),
            [
                vec![
                    "rsync",
                    "-a",
                    "--exclude=/logs/x.log",
                    "gpu:/srv/mirror/work/a/",
                    "/work/a/"
                ],
                vec!["rsync", "-a", "gpu:/srv/mirror/data/", "/data/"],
            ]
        );
        assert!(
            container(Runtime::Apptainer, false)
                .sync(&inv, log)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn remote_mounts_must_be_shell_safe() {
        // rsync hands remote paths to the remote shell unquoted.
        let tool = remote(container(Runtime::Apptainer, false));
        for bad in ["/work/a b", "/work/$x", "/work/a'b"] {
            let inv = Invocation::default().mount(bad);
            let err = tool.command(&inv).unwrap_err();
            assert!(matches!(&err, Error::Tools(m) if m.contains(bad)), "{err}");
            assert!(tool.sync(&inv, Path::new("/x.log")).is_err(), "{bad}");
        }
    }

    #[test]
    fn remote_spec_validation() {
        let base = format!("runtime = \"apptainer\"\nprogram = \"x\"\nimage = \"{IMAGE}\"");
        for (body, needle) in [
            (
                "runtime = \"native\"\nprogram = \"x\"\n[x.remote]\nhost = \"h\"\nroot = \"/r\""
                    .to_owned(),
                "remote",
            ),
            (
                format!("{base}\n[x.remote]\nhost = \"\"\nroot = \"/r\""),
                "host",
            ),
            (
                format!("{base}\n[x.remote]\nhost = \"h\"\nroot = \"r\""),
                "root",
            ),
            (
                format!("{base}\n[x.remote]\nhost = \"h\"\nroot = \"/r x\""),
                "root",
            ),
            (
                format!("{base}\n[x.remote]\nhost = \"h\"\nroot = \"/\""),
                "root",
            ),
            (format!("{base}\n[x.remote]\nhost = \"h\""), "root"),
            (
                format!("{base}\n[x.remote]\nhost = \"h\"\nroot = \"/r\"\nport = 22"),
                "port",
            ),
            (
                format!(
                    "{base}\n[x.remote]\nhost = \"h\"\nroot = \"/r\"\nenv = {{ \"A B\" = \"1\" }}"
                ),
                "env",
            ),
        ] {
            let err = tools_err(&format!("[x]\n{body}\n"));
            assert!(err.contains(needle), "{body:?}: {err}");
        }
        let t = tools(&format!(
            "[x]\n{base}\n[x.remote]\nhost = \"h\"\nroot = \"/r\"\nenv = {{ APPTAINER_CACHEDIR = \"/r/c\" }}\n"
        ))
        .unwrap();
        assert_eq!(t.get("x").unwrap().spec.remote.unwrap().host, "h");
    }

    #[test]
    fn tool_records_without_a_host_still_parse() {
        let json = r#"{"tool":"x","runtime":"native","image":null,"argv":["x"],"seconds":1.0,"log":"x.log"}"#;
        let rec: ToolRecord = serde_json::from_str(json).unwrap();
        assert_eq!(rec.host, None);
        assert!(!serde_json::to_string(&rec).unwrap().contains("host"));
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
            ("runtime = \"apptainer\"\nprogram = \"x\"", "image"),
            ("runtime = \"singularity\"\nprogram = \"x\"", "singularity"),
        ] {
            let err = tools_err(&format!("[x]\n{body}\n"));
            assert!(err.contains(needle), "{body:?}: {err}");
        }
        for runtime in ["podman", "apptainer"] {
            tools(&format!(
                "[x]\nruntime = \"{runtime}\"\nprogram = \"x\"\ngpu = true\n{digest}\n"
            ))
            .unwrap();
        }
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
                remote: None,
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
        for runtime in [Runtime::Docker, Runtime::Apptainer] {
            let v = container(runtime, false).version(&["--version"]).unwrap();
            assert_eq!(v, IMAGE);
        }
    }

    mod integration {
        use super::*;

        /// Needs `apptainer` and network access to Docker Hub on first run.
        #[test]
        fn apptainer_runs_colmap_help() {
            let dir = tempfile::tempdir().unwrap();
            let work = dir.path().canonicalize().unwrap();
            let log = work.join("colmap.log");
            let inv = Invocation::new(["help"]).mount(&work);
            let rec = container(Runtime::Apptainer, false)
                .run(&inv, &log)
                .unwrap();
            assert_eq!(rec.runtime, Runtime::Apptainer);
            let text = std::fs::read_to_string(&log).unwrap();
            assert!(text.contains("COLMAP"), "{text}");
        }

        /// Needs `SHORELINE_TEST_REMOTE_HOST` (ssh target with apptainer and
        /// rsync) and `SHORELINE_TEST_REMOTE_ROOT` (mirror directory there);
        /// `SHORELINE_TEST_REMOTE_CACHE` sets the remote `APPTAINER_CACHEDIR`.
        #[test]
        fn remote_apptainer_reads_and_writes_mirrored_mounts() {
            let var = |k: &str| std::env::var(k).unwrap_or_else(|_| panic!("{k} is not set"));
            let mut tool = container(Runtime::Apptainer, false);
            tool.spec.program = "sh".into();
            tool.spec.remote = Some(Remote {
                host: var("SHORELINE_TEST_REMOTE_HOST"),
                root: var("SHORELINE_TEST_REMOTE_ROOT").into(),
                env: std::env::var("SHORELINE_TEST_REMOTE_CACHE")
                    .map(|c| BTreeMap::from([("APPTAINER_CACHEDIR".to_owned(), c)]))
                    .unwrap_or_default(),
            });
            let dir = tempfile::tempdir().unwrap();
            let work = dir.path().canonicalize().unwrap();
            let (input, output) = (work.join("in"), work.join("out"));
            std::fs::create_dir_all(&input).unwrap();
            std::fs::create_dir_all(&output).unwrap();
            std::fs::write(input.join("a.txt"), "hello\n").unwrap();
            let script = format!(
                "cat {}/a.txt > {}/b.txt && uname -m",
                input.display(),
                output.display()
            );
            // Stages log inside their output mount; the log must keep the tool output.
            let log = output.join("logs/remote.log");
            let inv = Invocation::new(["-c", &script])
                .mount(&input)
                .mount(&output);
            let rec = tool.run(&inv, &log).unwrap();
            assert_eq!(rec.host, tool.spec.remote.as_ref().map(|r| r.host.clone()));
            assert_eq!(
                std::fs::read_to_string(output.join("b.txt")).unwrap(),
                "hello\n"
            );
            let text = std::fs::read_to_string(&log).unwrap();
            assert!(text.contains("$ rsync -a --delete"), "{text}");
            assert!(text.contains("x86_64"), "{text}");
        }
    }
}
