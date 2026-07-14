//! Offline simulators for training-pipeline development (no live streams).

pub mod microstructure;

pub use microstructure::{
    microstructure_obs_dim, BaselinePolicy, MicrostructureConfig, MicrostructureEvalStats,
    MicrostructureSim, MicrostructureStats, oracle_reward_estimate, run_baseline_episode,
    run_episode_with_actions,
};
