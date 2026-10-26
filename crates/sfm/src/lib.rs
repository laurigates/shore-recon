//! `sfm` stage: COLMAP / GLOMAP backends and COLMAP model parsing. (M2)

pub mod metrics;
pub mod model;

pub use model::Model;

pub const NAME: &str = "sfm";
