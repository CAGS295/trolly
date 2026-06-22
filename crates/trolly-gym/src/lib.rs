//! libtorch.rs training gym scaffold over trolly streams.
//!
//! Consumes normalized stream events for observations, steps with discrete
//! actions dispatched through [`trolly_strategy::StreamEgress`], and records
//! transitions in a replay ring buffer. Libtorch integration is behind the
//! `torch` feature flag.

mod action;
mod env;
pub mod games;
mod observation;
mod replay;

#[cfg(feature = "torch")]
pub mod libtorch;

#[cfg(feature = "torch")]
pub mod ppo;

#[cfg(feature = "torch")]
pub mod train;

pub use action::Action;
pub use env::{Env, EnvConfig, StepResult};
pub use observation::{features_from_event, FeatureVector, ObservationWindow};
pub use replay::{FeatureRingBuffer, ReplayBuffer, RolloutCollector, RolloutStep, Transition};

#[cfg(feature = "torch")]
pub use ppo::{
    losses_are_finite, ppo_loss, ActorCritic, OptimizerKind, PpoConfig, PpoLossBreakdown,
    PpoTrainer, RolloutBatch, WolfPpoConfig, WolfPpoTrainer,
};

#[cfg(feature = "torch")]
pub use games::{
    benchmark_ppo_vs_wolf, run_ppo_self_play, run_wolf_ppo_self_play, BenchmarkSummary,
    HarnessConfig, SelfPlayRunResult, DEFAULT_NUM_RUNS, DEFAULT_POLICY_UPDATES,
    DEFAULT_ROLLOUT_BATCH_SIZE, DEFAULT_TRACK_LAST_N,
};

#[cfg(feature = "torch")]
pub use train::{
    collect_env_rollout_step, load_checkpoint, rollout_collector_to_batch, save_checkpoint,
    CheckpointError, PpoTrainingDriver, TrainConfig, TrainStepMetrics, WolfPpoTrainingDriver,
};

/// Whether the crate was built with libtorch support.
pub fn torch_enabled() -> bool {
    cfg!(feature = "torch")
}

#[cfg(test)]
mod build_flag_tests {
    use super::torch_enabled;

    #[test]
    #[cfg(not(feature = "torch"))]
    fn torch_disabled_without_feature() {
        assert!(!torch_enabled());
    }

    #[test]
    #[cfg(feature = "torch")]
    fn torch_enabled_with_feature() {
        assert!(torch_enabled());
    }
}
