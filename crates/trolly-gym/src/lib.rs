//! libtorch.rs training gym scaffold over trolly streams.
//!
//! Consumes normalized stream events for observations, steps with discrete
//! actions dispatched through [`trolly_strategy::StreamEgress`], and records
//! transitions in a replay ring buffer. Libtorch integration is behind the
//! `torch` feature flag.

mod action;
mod env;
pub mod fingerprint;
mod observation;
mod replay;
pub mod sim;
pub mod ticks;
pub mod orchestrator;

#[cfg(feature = "torch")]
pub mod device;

#[cfg(feature = "torch")]
pub mod libtorch;

#[cfg(feature = "torch")]
pub mod ppo;

#[cfg(feature = "torch")]
pub mod games;

#[cfg(feature = "torch")]
pub mod train;

pub use action::Action;
pub use env::{Env, EnvConfig, StepResult};
pub use fingerprint::{
    load_sidecar, write_sidecar_for_checkpoint, ModelFingerprint, FINGERPRINT_SIDECAR,
};
pub use observation::{features_from_event, FeatureVector, ObservationWindow};
pub use replay::{
    FeatureRingBuffer, OnPolicyRolloutBuffer, OnPolicyStep, ReplayBuffer, Trajectory,
    TrajectoryReplay, Transition,
};
pub use ticks::{
    ensure_local_clickhouse, ingest_and_reload_sim, ClickHouseTicks, TickRow, TickTape,
    DEFAULT_CLICKHOUSE_URL,
};

/// Whether the crate was built with libtorch support.
pub fn torch_enabled() -> bool {
    cfg!(feature = "torch")
}
