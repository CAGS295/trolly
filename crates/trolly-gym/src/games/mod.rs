//! Offline two-player zero-sum matrix games (Ratcliffe et al. IEEE CoG 2019).

mod matrix;
mod metrics;

#[cfg(feature = "torch")]
mod harness;

pub use matrix::{MatrixGame, MatrixGameKind};
pub use metrics::{euclidean_distance, max_distance_over_last, mean_of_run_maxima};

#[cfg(feature = "torch")]
pub use harness::{
    benchmark_ppo_vs_wolf, run_ppo_self_play, run_wolf_ppo_self_play, BenchmarkSummary,
    HarnessConfig, SelfPlayRunResult, DEFAULT_NUM_RUNS, DEFAULT_POLICY_UPDATES,
    DEFAULT_ROLLOUT_BATCH_SIZE, DEFAULT_TRACK_LAST_N,
};
