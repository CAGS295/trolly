//! libtorch.rs training gym scaffold over trolly streams.
//!
//! Consumes normalized stream events for observations, steps with discrete
//! actions dispatched through [`trolly_strategy::StreamEgress`], and records
//! transitions in a replay ring buffer. Libtorch integration is behind the
//! `torch` feature flag.

mod action;
mod env;
mod observation;
mod replay;

#[cfg(feature = "torch")]
pub mod libtorch;

#[cfg(feature = "torch")]
pub mod ppo;

pub use action::Action;
pub use env::{Env, EnvConfig, StepResult};
pub use observation::{features_from_event, FeatureVector, ObservationWindow};
#[cfg(feature = "torch")]
pub use ppo::{
    compute_advantages, new_actor_critic, cpu_device, ActorCritic, PpoConfig, PpoTrainer,
    RolloutBatch, UpdateMetrics, WolfPpoConfig, WolfPpoTrainer,
};

/// Whether the crate was built with libtorch support.
pub fn torch_enabled() -> bool {
    cfg!(feature = "torch")
}
