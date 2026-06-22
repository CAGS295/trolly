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
pub use replay::{FeatureRingBuffer, ReplayBuffer, Transition};

#[cfg(feature = "torch")]
pub use ppo::{
    losses_are_finite, ppo_loss, ActorCritic, OptimizerKind, PpoConfig, PpoLossBreakdown,
    PpoTrainer, RolloutBatch, WolfPpoConfig, WolfPpoTrainer,
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
