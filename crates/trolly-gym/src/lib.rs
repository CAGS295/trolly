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
pub use games::{euclidean_distance, max_distance_last_n, mean_std, MatrixGame, MatrixGameKind};
pub use replay::{
    action_from_index, action_index, FeatureRingBuffer, OnPolicyRolloutBuffer, OnPolicyStep,
    ReplayBuffer, Transition,
};

#[cfg(feature = "torch")]
pub use games::{
    run_matrix_experiment, run_matrix_experiments, MatrixExperimentConfig, MatrixRunResult,
    MatrixTrainerKind, DEFAULT_LAST_N_POLICY_UPDATES, DEFAULT_NUM_RUNS, DEFAULT_POLICY_UPDATES,
};

#[cfg(feature = "torch")]
pub use ppo::{
    compute_advantages, cpu_device, default_hidden_layers, ActorCritic, PpoConfig, PpoLossBreakdown,
    PpoTrainer, RolloutBatch, WolfPpoConfig, WolfPpoTrainer,
};

#[cfg(feature = "torch")]
pub use train::{
    actor_critic_from_checkpoint, collect_env_rollout, load_checkpoint, rollout_buffer_to_batch,
    save_checkpoint, smoke_train_loop, CheckpointMeta, TrainMetricsLog, TrainStepMetrics,
    TrainingLoopConfig, WolfPpoTrainingDriver,
};

/// Whether the crate was built with libtorch support.
pub fn torch_enabled() -> bool {
    cfg!(feature = "torch")
}
