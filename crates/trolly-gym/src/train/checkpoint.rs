//! Actor–critic checkpoint save/load via `tch` [`VarStore`](tch::nn::VarStore).

use std::fmt;
use std::path::Path;

use tch::TchError;

use crate::ppo::WolfPpoTrainer;

/// Errors during checkpoint I/O.
#[derive(Debug)]
pub enum CheckpointError {
    Save(TchError),
    Load(TchError),
}

impl fmt::Display for CheckpointError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Save(e) => write!(f, "failed to save actor–critic checkpoint: {e}"),
            Self::Load(e) => write!(f, "failed to load actor–critic checkpoint: {e}"),
        }
    }
}

impl std::error::Error for CheckpointError {}

/// Save actor–critic weights from a WoLF-PPO trainer.
///
/// Format: binary [`tch::nn::VarStore::save`] file containing all named
/// parameters under the actor–critic MLP (`fc_*`, `policy`, `value` tensors).
/// Optimizer state and WoLF NES estimate are **not** persisted.
pub fn save_actor_critic_checkpoint(
    trainer: &WolfPpoTrainer,
    path: impl AsRef<Path>,
) -> Result<(), CheckpointError> {
    trainer
        .var_store()
        .save(path)
        .map_err(CheckpointError::Save)
}

/// Load actor–critic weights into an existing trainer (same architecture required).
pub fn load_actor_critic_checkpoint(
    trainer: &mut WolfPpoTrainer,
    path: impl AsRef<Path>,
) -> Result<(), CheckpointError> {
    trainer
        .var_store_mut()
        .load(path)
        .map_err(CheckpointError::Load)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tch::{kind::Kind, Tensor};

    use crate::ppo::WolfPpoConfig;

    fn forward_logits(trainer: &WolfPpoTrainer, obs: &Tensor) -> Tensor {
        tch::no_grad(|| trainer.actor_critic().forward(obs).0)
    }

    #[test]
    fn checkpoint_round_trip_restores_forward_pass() {
        tch::manual_seed(42);
        let obs = Tensor::from_slice(&[1.0_f32, 2.0, 3.0, 4.0])
            .view([1, 4])
            .to_kind(Kind::Float);

        let trainer_a = WolfPpoTrainer::new(4, 3, WolfPpoConfig::default());
        let logits_before = forward_logits(&trainer_a, &obs);

        let path = std::env::temp_dir().join(format!(
            "trolly-gym-checkpoint-{}",
            std::process::id()
        ));
        save_actor_critic_checkpoint(&trainer_a, &path).unwrap();

        let mut trainer_b = WolfPpoTrainer::new(4, 3, WolfPpoConfig::default());
        let logits_random = forward_logits(&trainer_b, &obs);
        assert!(logits_before != logits_random);

        load_actor_critic_checkpoint(&mut trainer_b, &path).unwrap();
        let logits_after = forward_logits(&trainer_b, &obs);
        assert!(logits_before.allclose(&logits_after, 1e-6, 1e-6, false));

        let _ = std::fs::remove_file(path);
    }
}
