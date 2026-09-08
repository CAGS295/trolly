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
#[cfg(feature = "ort")]
pub mod onnx;
pub mod orchestrator;
pub mod policy;
mod replay;
pub mod sim;
pub mod ticks;

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
pub use env::{
    run_offline_policy_harness, Env, EnvConfig, OfflinePolicyHarnessError, RewardConfig,
    StepActionSource, StepResult,
};
pub use fingerprint::{
    load_sidecar, write_sidecar_for_checkpoint, ModelFingerprint, FINGERPRINT_SIDECAR,
};
pub use observation::{features_from_event, FeatureVector, ObservationWindow};
#[cfg(feature = "ort")]
pub use onnx::{OnnxPolicy, OnnxPolicyError};
#[cfg(feature = "torch")]
pub use policy::CheckpointPolicy;
pub use policy::{CheckpointOrHoldPolicy, HoldPolicy, PolicyProvider};
pub use replay::{
    FeatureRingBuffer, OnPolicyRolloutBuffer, OnPolicyStep, ReplayBuffer, Trajectory,
    TrajectoryReplay, Transition,
};
pub use ticks::{
    ensure_local_clickhouse, ingest_and_reload_sim, require_clickhouse_reachable, ClickHouseTicks,
    TickRow, TickTape, DEFAULT_CLICKHOUSE_URL,
};

/// Whether the crate was built with libtorch support.
pub fn torch_enabled() -> bool {
    cfg!(feature = "torch")
}

/// Whether the crate was built with ONNX Runtime support.
pub fn ort_enabled() -> bool {
    cfg!(feature = "ort")
}
