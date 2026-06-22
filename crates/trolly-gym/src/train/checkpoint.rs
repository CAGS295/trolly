//! Actor–critic checkpoint save/load via `tch` VarStore (`.ot` format).

use std::path::Path;

use tch::nn;

use crate::ppo::{PpoTrainer, WolfPpoTrainer};

/// Errors from checkpoint I/O.
#[derive(Debug)]
pub enum CheckpointError {
    Io(std::io::Error),
    Torch(tch::TchError),
}

impl std::fmt::Display for CheckpointError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(err) => write!(f, "checkpoint I/O error: {err}"),
            Self::Torch(err) => write!(f, "checkpoint torch error: {err}"),
        }
    }
}

impl std::error::Error for CheckpointError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(err) => Some(err),
            Self::Torch(err) => Some(err),
        }
    }
}

impl From<std::io::Error> for CheckpointError {
    fn from(value: std::io::Error) -> Self {
        Self::Io(value)
    }
}

impl From<tch::TchError> for CheckpointError {
    fn from(value: tch::TchError) -> Self {
        Self::Torch(value)
    }
}

/// Save actor–critic weights to a libtorch VarStore file (`.ot` by convention).
pub fn save_checkpoint(vs: &nn::VarStore, path: impl AsRef<Path>) -> Result<(), CheckpointError> {
    vs.save(path)?;
    Ok(())
}

/// Load actor–critic weights from a libtorch VarStore file.
pub fn load_checkpoint(vs: &mut nn::VarStore, path: impl AsRef<Path>) -> Result<(), CheckpointError> {
    vs.load(path)?;
    Ok(())
}

impl PpoTrainer {
    /// Persist this trainer's actor–critic weights.
    pub fn save_checkpoint(&self, path: impl AsRef<Path>) -> Result<(), CheckpointError> {
        save_checkpoint(self.var_store(), path)
    }

    /// Restore actor–critic weights into this trainer.
    pub fn load_checkpoint(&mut self, path: impl AsRef<Path>) -> Result<(), CheckpointError> {
        load_checkpoint(self.var_store_mut(), path)
    }
}

impl WolfPpoTrainer {
    /// Persist this trainer's actor–critic weights.
    pub fn save_checkpoint(&self, path: impl AsRef<Path>) -> Result<(), CheckpointError> {
        save_checkpoint(self.var_store(), path)
    }

    /// Restore actor–critic weights into this trainer.
    pub fn load_checkpoint(&mut self, path: impl AsRef<Path>) -> Result<(), CheckpointError> {
        load_checkpoint(self.var_store_mut(), path)
    }
}

#[cfg(test)]
mod tests {
    use crate::ppo::{PpoConfig, PpoTrainer};
    use tch::{Device, Kind, Tensor};

    #[test]
    fn checkpoint_round_trip_preserves_forward_pass() {
        let config = PpoConfig {
            obs_dim: 4,
            num_actions: 3,
            ..Default::default()
        };
        tch::manual_seed(1234);
        let trainer = PpoTrainer::new(config.clone());
        let obs = Tensor::randn([2, config.obs_dim], (Kind::Float, Device::Cpu));
        let (values_before, logits_before) = trainer.actor_critic().forward(&obs);

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("actor_critic.ot");
        trainer.save_checkpoint(&path).unwrap();

        tch::manual_seed(9999);
        let mut restored = PpoTrainer::new(config);
        restored.load_checkpoint(&path).unwrap();

        let (values_after, logits_after) = restored.actor_critic().forward(&obs);
        assert!(values_before.allclose(&values_after, 1e-5, 1e-5, false));
        assert!(logits_before.allclose(&logits_after, 1e-5, 1e-5, false));
    }
}
