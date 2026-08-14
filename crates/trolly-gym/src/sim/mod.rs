//! Offline simulators for training-pipeline development (no live streams).

pub mod microstructure;

pub use microstructure::{
    microstructure_obs_dim, BaselinePolicy, MicrostructureConfig, MicrostructureEvalStats,
    MicrostructureSim, MicrostructureStats, oracle_reward_estimate, run_baseline_episode,
    run_episode_with_actions,
};

/// Record Hold-path book ticks from the existing sim (no parallel market model).
pub fn ingest_sim_ticks(
    config: &MicrostructureConfig,
    session_id: &str,
    steps: usize,
) -> Vec<crate::ticks::TickRow> {
    let mut sim = MicrostructureSim::new(config.clone());
    sim.reset();
    let mut rows = vec![sim.last_tick(session_id, "sim")];
    for _ in 0..steps {
        sim.step(crate::action::Action::Hold);
        rows.push(sim.last_tick(session_id, "sim"));
    }
    rows
}
