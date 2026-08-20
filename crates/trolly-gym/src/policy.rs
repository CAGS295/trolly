//! Policy providers for stream-backed environment stepping.

use crate::action::Action;

#[cfg(feature = "torch")]
use std::path::{Path, PathBuf};

/// Injectable action provider for [`crate::Env`] stepping.
pub trait PolicyProvider {
    fn act(&self, obs: &[f32]) -> Action;
}

/// Default policy that never changes inventory.
#[derive(Debug, Default, Clone, Copy)]
pub struct HoldPolicy;

impl PolicyProvider for HoldPolicy {
    fn act(&self, _obs: &[f32]) -> Action {
        Action::Hold
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
    #[cfg(feature = "torch")]
    Checkpoint(CheckpointPolicy),
    #[cfg(feature = "ort")]
    Onnx(crate::onnx::OnnxPolicy),
}

impl CheckpointOrHoldPolicy {
    pub fn hold() -> Self {
        Self::Hold(HoldPolicy)
    }

    #[cfg(feature = "torch")]
    pub fn checkpoint(policy: CheckpointPolicy) -> Self {
        Self::Checkpoint(policy)
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
            #[cfg(feature = "torch")]
            Self::Checkpoint(policy) => policy.act(obs),
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
}
