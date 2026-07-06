//! Offline simulators for training-pipeline development (no live streams).

pub mod microstructure;

pub use microstructure::{
    microstructure_obs_dim, MicrostructureConfig, MicrostructureSim, MicrostructureStats,
};
