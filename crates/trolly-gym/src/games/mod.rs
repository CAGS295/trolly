//! Offline matrix-game validation harness for WoLF-PPO (WP-019).

mod matrix;
mod metrics;

#[cfg(feature = "torch")]
mod harness;

pub use matrix::{MatrixGame, MatrixGameKind};
pub use metrics::{euclidean_distance, max_distance_last_n, mean_std};

#[cfg(feature = "torch")]
pub use harness::{
    run_matrix_experiment, run_matrix_experiments, MatrixExperimentConfig, MatrixRunResult,
    MatrixTrainerKind, DEFAULT_LAST_N_POLICY_UPDATES, DEFAULT_NUM_RUNS, DEFAULT_POLICY_UPDATES,
};
