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

pub use action::{Action, ActionDecision};
pub use env::{
    run_offline_policy_harness, Env, EnvConfig, OfflinePolicyHarnessError, RewardConfig,
    StepActionSource, StepResult,
};
pub use fingerprint::{
    load_sidecar, write_sidecar_for_checkpoint, ModelFingerprint, FINGERPRINT_SIDECAR,
};
pub use observation::{
    features_from_event, join_feature_frames, zero_stream_features, FeatureVector,
    ObservationWindow, STREAM_FEATURES,
};
pub use onnx::{
    inspect_gaussian_mu_onnx, path_is_retired_unit_lot, write_recorded_mean_mu_onnx,
    GaussianMuOnnxInfo, OnnxExportError, DEFAULT_GAUSSIAN_MU_ONNX_OBS_DIM, GAUSSIAN_MU_ONNX_INPUT,
    GAUSSIAN_MU_ONNX_OUTPUT,
};
#[cfg(feature = "ort")]
pub use onnx::{OnnxGaussianMeanPolicy, OnnxPolicy, OnnxPolicyError};
#[cfg(feature = "torch")]
pub use policy::CheckpointPolicy;
pub use policy::{
    decode_gaussian_mean_output, CheckpointOrHoldPolicy, DispatchSymbolPolicy,
    GaussianMeanDecodeError, HoldPolicy, PolicyProvider, QuantizeInventoryPolicy,
};
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
