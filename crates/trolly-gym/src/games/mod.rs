//! Offline two-player zero-sum matrix games for WoLF-PPO validation (WP-019).

mod matrix;
mod nes;
mod self_play;

pub use matrix::{MatrixGame, MatrixGameKind};
pub use nes::{max_distance_over_last_n, mean_max_distance_last_n, NesPolicy};
pub use self_play::{
    policy_probabilities, run_benchmark, run_self_play, MatrixBenchmarkResult, MatrixExperimentConfig,
    MatrixRunResult, MatrixTrainerKind,
};
