//! Actor–critic checkpoint save/load (PyTorch VarStore + JSON sidecar).

use std::fs;
use std::path::{Path, PathBuf};

use tch::nn;

use crate::ppo::{default_hidden_layers, ActorCritic, PpoConfig};

/// Sidecar metadata written next to the `.pt` weight file.
///
/// ## File format (v1)
///
/// Given base path `model` (no extension):
///
/// - `model.safetensors` — actor–critic weight tensors (safetensors via `tch`).
/// - `model.meta.json` — this struct as JSON (`format_version`, dims, hidden layers).
///
/// Round-trip: save from a trained [`ActorCritic`] var-store, load into a fresh
/// var-store with matching architecture, forward pass outputs match on CPU.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct CheckpointMeta {
    pub format_version: u32,
    pub obs_dim: i64,
    pub action_count: i64,
    pub hidden_layers: Vec<i64>,
}

impl CheckpointMeta {
    pub fn new(obs_dim: i64, action_count: i64, hidden_layers: Vec<i64>) -> Self {
        Self {
            format_version: 1,
            obs_dim,
            action_count,
            hidden_layers,
        }
    }

    pub fn with_default_hidden(obs_dim: i64, action_count: i64) -> Self {
        Self::new(obs_dim, action_count, default_hidden_layers())
    }
}

/// Resolved paths for a checkpoint base name.
#[derive(Debug, Clone)]
pub struct CheckpointPaths {
    pub weights: PathBuf,
    pub meta: PathBuf,
}

impl CheckpointPaths {
    pub fn from_base(base: impl AsRef<Path>) -> Self {
        let base = base.as_ref();
        Self {
            weights: base.with_extension("safetensors"),
            meta: base.with_extension("meta.json"),
        }
    }
}

/// Save actor–critic weights and metadata.
pub fn save_checkpoint(
    base: impl AsRef<Path>,
    vs: &nn::VarStore,
    meta: &CheckpointMeta,
) -> Result<CheckpointPaths, Box<dyn std::error::Error>> {
    let paths = CheckpointPaths::from_base(base.as_ref());
    if let Some(parent) = paths.weights.parent() {
        fs::create_dir_all(parent)?;
    }
    vs.save(&paths.weights)?;
    let json = serde_json::to_string_pretty(meta)?;
    fs::write(&paths.meta, json)?;
    Ok(paths)
}

/// Load weights into `vs` after building an actor–critic with matching architecture.
///
/// Variables must be registered (via [`actor_critic_from_checkpoint`]) before load.
pub fn load_checkpoint(
    base: impl AsRef<Path>,
    vs: &mut nn::VarStore,
    config: &PpoConfig,
) -> Result<(CheckpointMeta, ActorCritic), Box<dyn std::error::Error>> {
    let paths = CheckpointPaths::from_base(base.as_ref());
    let json = fs::read_to_string(&paths.meta)?;
    let meta: CheckpointMeta = serde_json::from_str(&json)?;
    if meta.format_version != 1 {
        return Err(format!("unsupported checkpoint format version {}", meta.format_version).into());
    }
    let model = actor_critic_from_checkpoint(vs, &meta, config);
    vs.load(&paths.weights)?;
    Ok((meta, model))
}

/// Reconstruct an [`ActorCritic`] from a loaded var-store and metadata.
pub fn actor_critic_from_checkpoint(
    vs: &nn::VarStore,
    meta: &CheckpointMeta,
    config: &PpoConfig,
) -> ActorCritic {
    ActorCritic::new(
        &vs.root(),
        meta.obs_dim,
        meta.action_count,
        &meta.hidden_layers,
        config,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use tch::{nn, Device, Kind, Tensor};

    #[test]
    fn checkpoint_roundtrip_restores_forward_pass() {
        let device = Device::Cpu;
        let obs_dim = 4_i64;
        let action_count = 3_i64;
        let config = PpoConfig::default();

        let vs1 = nn::VarStore::new(device);
        let model1 = ActorCritic::new(
            &vs1.root(),
            obs_dim,
            action_count,
            &default_hidden_layers(),
            &config,
        );

        let obs = Tensor::randn(&[2, obs_dim], (Kind::Float, device));
        let (logits1, values1) = model1.forward(&obs);

        let dir = std::env::temp_dir().join(format!(
            "trolly_gym_ckpt_{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let base = dir.join("actor_critic");

        let meta = CheckpointMeta::with_default_hidden(obs_dim, action_count);
        save_checkpoint(&base, &vs1, &meta).unwrap();

        let mut vs2 = nn::VarStore::new(device);
        let (_loaded_meta, model2) =
            load_checkpoint(&base, &mut vs2, &config).unwrap();
        let (logits2, values2) = model2.forward(&obs);

        let diff_logits = (&logits1 - &logits2).abs().max().double_value(&[]);
        let diff_values = (&values1 - &values2).abs().max().double_value(&[]);
        assert!(diff_logits < 1e-6, "logits diff {diff_logits}");
        assert!(diff_values < 1e-6, "values diff {diff_values}");

        let _ = fs::remove_dir_all(&dir);
    }
}
