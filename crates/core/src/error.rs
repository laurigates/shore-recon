use std::path::{Path, PathBuf};

pub type Result<T, E = Error> = std::result::Result<T, E>;

/// `map_err` adapter that attaches `path` to an I/O error.
pub fn io_error(path: &Path) -> impl FnOnce(std::io::Error) -> Error + use<> {
    let path = path.to_owned();
    move |source| Error::Io { path, source }
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("failed to read {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to parse {path}: {source}")]
    Parse {
        path: PathBuf,
        #[source]
        source: Box<toml::de::Error>,
    },
    #[error("invalid config: {0}")]
    Config(String),
    #[error("stage graph: {0}")]
    Dag(String),
    #[error("quality gate failed: {0}")]
    Gate(String),
    #[error("stage cache: {0}")]
    Cache(String),
    /// A stage could not produce its output (bad input data, nothing kept).
    #[error("{stage}: {message}")]
    Stage {
        stage: &'static str,
        message: String,
    },
    #[error("tools config: {0}")]
    Tools(String),
    #[error("failed to start {program}: {source}")]
    Spawn {
        program: String,
        #[source]
        source: std::io::Error,
    },
    #[error("{tool} failed ({status}); see {log}")]
    ToolFailed {
        tool: String,
        status: std::process::ExitStatus,
        log: PathBuf,
    },
    /// The remote command of a detached run (ADR-0009, decision 6) exited non-zero.
    #[error("{tool} failed on the remote host (exit {code}); see {log}")]
    RemoteToolFailed {
        tool: String,
        code: i32,
        log: PathBuf,
    },
    /// Reconnecting to the host of a detached run gave up; the command may live on.
    #[error(
        "ssh connection to {host} was lost after {attempts} reconnect attempts; {tool} may still \
         be running on the host in {run_dir} (see out.log and status there); local log {log}"
    )]
    RemoteLost {
        tool: String,
        host: String,
        run_dir: PathBuf,
        attempts: u32,
        log: PathBuf,
    },
    /// A detached run that can no longer finish: its run directory is gone, or
    /// its command ended without recording an exit code.
    #[error("{tool} on {host}: {message}; see {log}")]
    RemoteRun {
        tool: String,
        host: String,
        message: String,
        log: PathBuf,
    },
}
