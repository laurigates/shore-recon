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
}
