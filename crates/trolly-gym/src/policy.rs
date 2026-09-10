//! Policy providers for stream-backed environment stepping.

use std::sync::atomic::{AtomicUsize, Ordering};

use crate::action::Action;

#[cfg(feature = "torch")]
use std::path::{Path, PathBuf};

/// Deadzone used by [`Action::quantize_inventory`] when none is supplied.
pub const DEFAULT_INVENTORY_DEADZONE: f32 = 0.25;

/// Injectable action provider for [`crate::Env`] stepping.
pub trait PolicyProvider {
    fn act(&self, obs: &[f32]) -> Action;
}

/// Wrap a target-inventory source and quantize onto `{Hold,Buy,Sell}`.
///
/// Used to join WP-033/WP-034 Gaussian `a ∈ (-1, 1)` onto the live
/// [`Action::dispatch`] path without a parallel order builder.
#[derive(Debug, Clone)]
pub struct QuantizeInventoryPolicy<F> {
    choose_target: F,
    hold_deadzone: f32,
}

impl<F> QuantizeInventoryPolicy<F>
where
    F: Fn(&[f32]) -> f32,
{
    pub fn new(choose_target: F, hold_deadzone: f32) -> Self {
        Self {
            choose_target,
            hold_deadzone,
        }
    }
}

impl<F> PolicyProvider for QuantizeInventoryPolicy<F>
where
    F: Fn(&[f32]) -> f32,
{
    fn act(&self, obs: &[f32]) -> Action {
        Action::quantize_inventory((self.choose_target)(obs), self.hold_deadzone)
    }
}

/// Default policy that never changes inventory.
#[derive(Debug, Default, Clone, Copy)]
pub struct HoldPolicy;

impl PolicyProvider for HoldPolicy {
    fn act(&self, _obs: &[f32]) -> Action {
        Action::Hold
    }
}

/// Parse a recorded tanh-Gaussian mean-action vector (`a ∈ [-1, 1]`).
///
/// Accepts comma- and/or whitespace-separated floats. Used by the offline
/// `execute policy-demo` path so Gaussian inventory can be quantized without
/// libtorch.
pub fn parse_mean_action_targets(spec: &str) -> Result<Vec<f32>, String> {
    let trimmed = spec.trim();
    if trimmed.is_empty() {
        return Err("empty mean-action vector".into());
    }
    trimmed
        .split(|c: char| c == ',' || c.is_whitespace())
        .filter(|part| !part.is_empty())
        .map(|part| {
            part.parse::<f32>()
                .map_err(|err| format!("invalid mean action {part:?}: {err}"))
        })
        .collect()
}

/// Replay a recorded mean-action tape and quantize onto `{Hold,Buy,Sell}`.
///
/// Each [`PolicyProvider::act`] call consumes the next target. Exhausted tapes
/// hold (`a = 0`). This is the default-build stand-in for a
/// `microstructure/gaussian_mlp` mean-action eval.
#[derive(Debug)]
pub struct RecordedMeanActionPolicy {
    targets: Vec<f32>,
    next: AtomicUsize,
    hold_deadzone: f32,
}

impl RecordedMeanActionPolicy {
    pub fn new(targets: impl Into<Vec<f32>>, hold_deadzone: f32) -> Self {
        Self {
            targets: targets.into(),
            next: AtomicUsize::new(0),
            hold_deadzone,
        }
    }

    pub fn parse(spec: &str, hold_deadzone: f32) -> Result<Self, String> {
        Ok(Self::new(parse_mean_action_targets(spec)?, hold_deadzone))
    }

    pub fn hold_deadzone(&self) -> f32 {
        self.hold_deadzone
    }

    fn next_target(&self) -> f32 {
        let idx = self.next.fetch_add(1, Ordering::Relaxed);
        self.targets.get(idx).copied().unwrap_or(0.0)
    }
}

impl PolicyProvider for RecordedMeanActionPolicy {
    fn act(&self, _obs: &[f32]) -> Action {
        Action::quantize_inventory(self.next_target(), self.hold_deadzone)
    }
}

impl<F> PolicyProvider for F
where
    F: Fn(&[f32]) -> Action,
{
    fn act(&self, obs: &[f32]) -> Action {
        self(obs)
    }
}

/// Runtime policy choice for offline execution harnesses.
///
/// Default builds always have the safe hold branch. Torch builds can replace it
/// with a checkpoint-backed actor-critic loaded from `latest.safetensors`. ONNX
/// Runtime builds can load an exported actor model for inference without
/// linking libtorch.
pub enum CheckpointOrHoldPolicy {
    Hold(HoldPolicy),
    QuantizedMean(RecordedMeanActionPolicy),
    #[cfg(feature = "torch")]
    Checkpoint(CheckpointPolicy),
    #[cfg(feature = "torch")]
    GaussianCheckpoint(GaussianCheckpointPolicy),
    #[cfg(feature = "ort")]
    Onnx(crate::onnx::OnnxPolicy),
}

impl CheckpointOrHoldPolicy {
    pub fn hold() -> Self {
        Self::Hold(HoldPolicy)
    }

    pub fn quantized_mean(policy: RecordedMeanActionPolicy) -> Self {
        Self::QuantizedMean(policy)
    }

    pub fn from_mean_actions(targets: impl Into<Vec<f32>>, hold_deadzone: f32) -> Self {
        Self::QuantizedMean(RecordedMeanActionPolicy::new(targets, hold_deadzone))
    }

    pub fn from_mean_actions_csv(spec: &str, hold_deadzone: f32) -> Result<Self, String> {
        RecordedMeanActionPolicy::parse(spec, hold_deadzone).map(Self::QuantizedMean)
    }

    #[cfg(feature = "torch")]
    pub fn checkpoint(policy: CheckpointPolicy) -> Self {
        Self::Checkpoint(policy)
    }

    #[cfg(feature = "torch")]
    pub fn gaussian_checkpoint(policy: GaussianCheckpointPolicy) -> Self {
        Self::GaussianCheckpoint(policy)
    }

    #[cfg(feature = "torch")]
    pub fn from_latest_gaussian_checkpoint_dir(
        dir: impl AsRef<Path>,
        hold_deadzone: f32,
    ) -> Result<Self, String> {
        GaussianCheckpointPolicy::from_latest_checkpoint_dir(dir, hold_deadzone)
            .map(Self::GaussianCheckpoint)
    }

    #[cfg(feature = "ort")]
    pub fn onnx(policy: crate::onnx::OnnxPolicy) -> Self {
        Self::Onnx(policy)
    }

    #[cfg(feature = "torch")]
    pub fn from_latest_checkpoint_dir(
        dir: impl AsRef<Path>,
        obs_dim: i64,
        config: crate::ppo::PpoConfig,
    ) -> Result<Self, String> {
        CheckpointPolicy::from_latest_checkpoint_dir(dir, obs_dim, config).map(Self::Checkpoint)
    }

    #[cfg(feature = "ort")]
    pub fn from_onnx_model(
        path: impl AsRef<std::path::Path>,
        obs_dim: i64,
    ) -> Result<Self, crate::onnx::OnnxPolicyError> {
        crate::onnx::OnnxPolicy::from_model(path, obs_dim).map(Self::Onnx)
    }
}

impl Default for CheckpointOrHoldPolicy {
    fn default() -> Self {
        Self::hold()
    }
}

impl PolicyProvider for CheckpointOrHoldPolicy {
    fn act(&self, obs: &[f32]) -> Action {
        match self {
            Self::Hold(policy) => policy.act(obs),
            Self::QuantizedMean(policy) => policy.act(obs),
            #[cfg(feature = "torch")]
            Self::Checkpoint(policy) => policy.act(obs),
            #[cfg(feature = "torch")]
            Self::GaussianCheckpoint(policy) => policy.act(obs),
            #[cfg(feature = "ort")]
            Self::Onnx(policy) => policy.act(obs),
        }
    }
}

/// Policy backed by a saved torch actor-critic checkpoint.
#[cfg(feature = "torch")]
pub struct CheckpointPolicy {
    _vs: tch::nn::VarStore,
    model: crate::ppo::ActorCritic,
    obs_dim: i64,
}

#[cfg(feature = "torch")]
impl CheckpointPolicy {
    /// Load `latest.safetensors` from a checkpoint directory.
    pub fn from_latest_checkpoint_dir(
        dir: impl AsRef<Path>,
        obs_dim: i64,
        config: crate::ppo::PpoConfig,
    ) -> Result<Self, String> {
        let path = dir.as_ref().join(crate::train::LATEST_CHECKPOINT);
        Self::from_checkpoint(path, obs_dim, config)
    }

    /// Load a concrete actor-critic checkpoint.
    pub fn from_checkpoint(
        path: impl Into<PathBuf>,
        obs_dim: i64,
        config: crate::ppo::PpoConfig,
    ) -> Result<Self, String> {
        let path = path.into();
        let mut vs = tch::nn::VarStore::new(tch::Device::Cpu);
        let model = crate::ppo::ActorCritic::new(&vs, obs_dim, Action::COUNT, &config);
        crate::train::load_checkpoint(&mut vs, &path)
            .map_err(|err| format!("load checkpoint {}: {err}", path.display()))?;
        Ok(Self {
            _vs: vs,
            model,
            obs_dim,
        })
    }
}

#[cfg(feature = "torch")]
impl PolicyProvider for CheckpointPolicy {
    fn act(&self, obs: &[f32]) -> Action {
        let mut padded = vec![0.0_f32; self.obs_dim.max(0) as usize];
        for (dst, src) in padded.iter_mut().zip(obs.iter().copied()) {
            *dst = src;
        }
        let input = tch::Tensor::from_slice(&padded).view([1, self.obs_dim]);
        let _guard = tch::no_grad_guard();
        let (logits, _) = self.model.forward(&input);
        Action::from_index(logits.argmax(-1, false).int64_value(&[0]))
    }
}

/// Tanh-Gaussian ladder checkpoint (`microstructure/gaussian_mlp` or
/// `gaussian_liquid`). Mean action `tanh(μ)` is quantized via WP-035.
#[cfg(feature = "torch")]
pub struct GaussianCheckpointPolicy {
    _vs: tch::nn::VarStore,
    model: crate::train::GaussianPolicy,
    obs_dim: i64,
    hold_deadzone: f32,
}

#[cfg(feature = "torch")]
impl GaussianCheckpointPolicy {
    /// Load `latest.safetensors` from a Gaussian ladder checkpoint directory.
    ///
    /// Refuses `_retired_unit_lot_microstructure`. Architecture is inferred
    /// from the path (`gaussian_liquid` vs default `gaussian_mlp`).
    pub fn from_latest_checkpoint_dir(
        dir: impl AsRef<Path>,
        hold_deadzone: f32,
    ) -> Result<Self, String> {
        let dir = dir.as_ref();
        if crate::train::is_retired_unit_lot_dir(dir) {
            return Err(format!(
                "refusing retired unit-lot checkpoint {}",
                dir.display()
            ));
        }
        let path = dir.join(crate::train::LATEST_CHECKPOINT);
        Self::from_checkpoint(path, gaussian_architecture_from_path(dir), hold_deadzone)
    }

    pub fn from_checkpoint(
        path: impl Into<PathBuf>,
        architecture: crate::train::GaussianArchitecture,
        hold_deadzone: f32,
    ) -> Result<Self, String> {
        let path = path.into();
        if crate::train::is_retired_unit_lot_dir(&path) {
            return Err(format!(
                "refusing retired unit-lot checkpoint {}",
                path.display()
            ));
        }
        let sim = crate::sim::MicrostructureConfig::default();
        let obs_dim = sim.ladder_obs_dim();
        let mut vs = tch::nn::VarStore::new(tch::Device::Cpu);
        let config = crate::ppo::PpoConfig::default();
        let model = match architecture {
            crate::train::GaussianArchitecture::Mlp => crate::train::GaussianPolicy::Mlp(
                crate::ppo::GaussianActorCritic::new(&vs, obs_dim, &config),
            ),
            crate::train::GaussianArchitecture::LiquidRungs => {
                crate::train::GaussianPolicy::LiquidRungs(crate::ppo::RungLiquidGaussian::new(
                    &vs,
                    obs_dim,
                    sim.rung_count as i64,
                    &config,
                ))
            }
        };
        crate::train::load_checkpoint(&mut vs, &path)
            .map_err(|err| format!("load gaussian checkpoint {}: {err}", path.display()))?;
        Ok(Self {
            _vs: vs,
            model,
            obs_dim,
            hold_deadzone,
        })
    }
}

#[cfg(feature = "torch")]
fn gaussian_architecture_from_path(path: &Path) -> crate::train::GaussianArchitecture {
    let text = path.to_string_lossy();
    if text.contains(crate::train::GAUSSIAN_LIQUID_ARCH) {
        crate::train::GaussianArchitecture::LiquidRungs
    } else {
        crate::train::GaussianArchitecture::Mlp
    }
}

#[cfg(feature = "torch")]
impl PolicyProvider for GaussianCheckpointPolicy {
    fn act(&self, obs: &[f32]) -> Action {
        let mut padded = vec![0.0_f32; self.obs_dim.max(0) as usize];
        for (dst, src) in padded.iter_mut().zip(obs.iter().copied()) {
            *dst = src;
        }
        let input = tch::Tensor::from_slice(&padded).view([1, self.obs_dim]);
        let _guard = tch::no_grad_guard();
        let target = crate::sim::clamp_inventory_target(
            self.model.mean_action(&input).double_value(&[]) as f32,
        );
        Action::quantize_inventory(target, self.hold_deadzone)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hold_policy_returns_hold() {
        assert_eq!(HoldPolicy.act(&[1.0, 2.0]), Action::Hold);
    }

    #[test]
    fn checkpoint_or_hold_defaults_to_hold() {
        let policy = CheckpointOrHoldPolicy::default();
        assert_eq!(policy.act(&[1.0, 2.0]), Action::Hold);
    }

    #[test]
    fn quantize_inventory_policy_maps_targets() {
        let policy = QuantizeInventoryPolicy::new(|obs: &[f32]| obs[0], 0.25);
        assert_eq!(policy.act(&[0.1]), Action::Hold);
        assert_eq!(policy.act(&[0.4]), Action::Buy);
        assert_eq!(policy.act(&[-0.4]), Action::Sell);
    }

    #[test]
    fn recorded_mean_actions_quantize_in_order() {
        let policy =
            RecordedMeanActionPolicy::parse("0.8,-0.8,0.05", DEFAULT_INVENTORY_DEADZONE).unwrap();
        assert_eq!(policy.act(&[0.0]), Action::Buy);
        assert_eq!(policy.act(&[0.0]), Action::Sell);
        assert_eq!(policy.act(&[0.0]), Action::Hold);
        assert_eq!(policy.act(&[0.0]), Action::Hold);
    }

    #[test]
    fn parse_mean_action_targets_rejects_empty() {
        assert!(parse_mean_action_targets("  ").is_err());
        assert!(parse_mean_action_targets("0.8,nope").is_err());
    }

    #[test]
    fn checkpoint_or_hold_from_mean_actions_csv() {
        let policy =
            CheckpointOrHoldPolicy::from_mean_actions_csv("0.9", DEFAULT_INVENTORY_DEADZONE)
                .unwrap();
        assert_eq!(policy.act(&[]), Action::Buy);
    }

    #[test]
    fn closure_policy_can_return_actions() {
        let policy = |obs: &[f32]| {
            if obs.first().copied().unwrap_or_default() > 0.0 {
                Action::Buy
            } else {
                Action::Sell
            }
        };
        assert_eq!(policy.act(&[1.0]), Action::Buy);
        assert_eq!(policy.act(&[-1.0]), Action::Sell);
    }

    #[cfg(feature = "torch")]
    #[test]
    fn checkpoint_policy_loads_latest_and_acts() {
        use crate::ppo::{ActorCritic, PpoConfig};
        use crate::train::{save_checkpoint, LATEST_CHECKPOINT};
        use tch::{nn, Device};

        let obs_dim = 7_i64;
        let config = PpoConfig::default();
        let vs = nn::VarStore::new(Device::Cpu);
        let _model = ActorCritic::new(&vs, obs_dim, Action::COUNT, &config);

        let dir = std::env::temp_dir().join(format!(
            "trolly_gym_checkpoint_policy_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        save_checkpoint(&vs, dir.join(LATEST_CHECKPOINT)).unwrap();

        let policy = CheckpointPolicy::from_latest_checkpoint_dir(&dir, obs_dim, config).unwrap();
        let action = policy.act(&vec![0.0; obs_dim as usize]);
        assert!(matches!(action, Action::Hold | Action::Buy | Action::Sell));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(feature = "torch")]
    #[test]
    fn gaussian_checkpoint_policy_loads_mlp_and_quantizes() {
        use crate::ppo::{GaussianActorCritic, PpoConfig};
        use crate::train::{save_checkpoint, LATEST_CHECKPOINT};
        use tch::{nn, Device};

        let sim = crate::sim::MicrostructureConfig::default();
        let obs_dim = sim.ladder_obs_dim();
        let vs = nn::VarStore::new(Device::Cpu);
        let _model = GaussianActorCritic::new(&vs, obs_dim, &PpoConfig::default());

        let dir = std::env::temp_dir().join(format!(
            "trolly_gym_gaussian_mlp_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        save_checkpoint(&vs, dir.join(LATEST_CHECKPOINT)).unwrap();

        let policy =
            GaussianCheckpointPolicy::from_latest_checkpoint_dir(&dir, DEFAULT_INVENTORY_DEADZONE)
                .unwrap();
        let action = policy.act(&vec![0.0; 7]);
        assert!(matches!(action, Action::Hold | Action::Buy | Action::Sell));

        let retired = dir.join(crate::train::RETIRED_UNIT_LOT_MICROSTRUCTURE);
        assert!(GaussianCheckpointPolicy::from_latest_checkpoint_dir(
            &retired,
            DEFAULT_INVENTORY_DEADZONE
        )
        .is_err());

        let _ = std::fs::remove_dir_all(&dir);
    }
}
