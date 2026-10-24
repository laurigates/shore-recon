//! Core building blocks for shoreline-recon: configuration, stage DAG,
//! fingerprinting, the [`Stage`] trait, the stage cache and pipeline runner,
//! and external tool execution.

pub mod cache;
pub mod config;
pub mod dag;
pub mod error;
pub mod fingerprint;
pub mod pipeline;
pub mod stage;
pub mod tools;

pub use config::Config;
pub use error::{Error, Result};
pub use fingerprint::Fingerprint;
pub use stage::Stage;
pub use tools::{Tool, Tools};
