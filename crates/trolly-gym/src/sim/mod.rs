//! Offline simulators for training-pipeline development (no live streams).

pub mod microstructure;

pub use microstructure::{
    generate_resampled_tick_rows, ladder_obs_dim, microstructure_obs_dim, oracle_reward_estimate,
    run_baseline_episode, run_episode_with_actions, BaselinePolicy, DepthLadderSpec,
    MicrostructureConfig, MicrostructureEvalStats, MicrostructureSim, MicrostructureStats,
    LADDER_FEATURES_PER_RUNG,
};

/// Record Hold-path book ticks from several mid-path seeds (WP-032).
///
/// A single 64-tick Hold tape is no longer the only mid path: `steps` is split
/// across resampled episodes so ingest matches training seed diversity.
pub fn ingest_sim_ticks(
    config: &MicrostructureConfig,
    session_id: &str,
    steps: usize,
) -> Vec<crate::ticks::TickRow> {
    let episode_count = 4.max(steps / 16).min(steps.max(1));
    let steps_per = (steps / episode_count).max(1);
    generate_resampled_tick_rows(config, session_id, episode_count, steps_per)
}
