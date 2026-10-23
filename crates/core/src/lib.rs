//! Core building blocks for shoreline-recon: configuration, stage DAG,
//! fingerprinting and the [`Stage`] trait.

pub mod config;
pub mod dag;
pub mod error;
pub mod fingerprint;
pub mod stage;

pub use config::Config;
pub use error::{Error, Result};
pub use fingerprint::Fingerprint;
pub use stage::Stage;
