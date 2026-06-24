//! Checkpoint save and load for actor-critic networks.
//!
//! # File format
//!
//! Two formats are supported, selected by the file extension:
//!
//! | Extension | Format | Notes |
//! |-----------|--------|-------|
//! | `.safetensors` | [SafeTensors](https://github.com/huggingface/safetensors) | Recommended. Cross-language; readable by Python `safetensors` library. |
//! | anything else (e.g. `.ckpt`) | libtorch C++ module format | Binary; tch-native. Avoid `.pt` and `.bin` — those extensions trigger a pickle-format loader that is incompatible with what `VarStore::save` writes for non-safetensors paths. |
//!
//! **Recommendation:** use `.safetensors` extension for portability.
//!
//! # Round-trip guarantee
//!
//! A checkpoint written for a network of architecture (obs_dim, num_actions,
//! hidden_sizes) can only be loaded into a network **with the same
//! architecture** because the variable names and shapes must match exactly.
//! Record the architecture parameters alongside the file (e.g. in a sidecar
//! JSON).
//!
//! # Example
//!
//! ```rust,ignore
//! use trolly_gym::train::checkpoint::{save_checkpoint, load_checkpoint};
//! use trolly_gym::ppo::{PpoTrainer, PpoConfig};
//!
//! let mut trainer = PpoTrainer::new(4, 3, PpoConfig::default());
//! // … training …
//! save_checkpoint(&trainer.vs, "/tmp/model.safetensors").unwrap();
//!
//! // Restore into a fresh network with the same architecture:
//! let mut trainer2 = PpoTrainer::new(4, 3, PpoConfig::default());
//! load_checkpoint(&mut trainer2.vs, "/tmp/model.safetensors").unwrap();
//! ```

use std::path::Path;

use tch::nn;

/// Save all named variables in `vs` to `path`.
///
/// Use `.safetensors` extension for cross-language portability.
/// Avoid `.pt` / `.bin` extensions — tch's loader applies a different
/// (incompatible) deserialiser for those extensions.
pub fn save_checkpoint(vs: &nn::VarStore, path: impl AsRef<Path>) -> Result<(), tch::TchError> {
    vs.save(path)
}

/// Load named variables from `path` into `vs`.
///
/// Variable names and tensor shapes must exactly match those in `vs`.
/// The extension must match what was used during [`save_checkpoint`]:
/// `.safetensors` files are read with the safetensors deserialiser;
/// all other extensions use the libtorch C++ module format.
pub fn load_checkpoint(vs: &mut nn::VarStore, path: impl AsRef<Path>) -> Result<(), tch::TchError> {
    vs.load(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ppo::{ActorCritic, PpoConfig};
    use tch::{Device, Kind, Tensor, nn};

    fn make_ac(obs_dim: i64, num_actions: i64) -> (nn::VarStore, ActorCritic) {
        let vs = nn::VarStore::new(Device::Cpu);
        let ac = ActorCritic::new(&vs, obs_dim, num_actions, &PpoConfig::default());
        (vs, ac)
    }

    fn forward_flat(ac: &ActorCritic, obs: &Tensor) -> Vec<f64> {
        let _g = tch::no_grad_guard();
        let (logits, values) = ac.forward(obs);
        let mut out: Vec<f64> = logits.view([-1]).iter::<f64>().unwrap().collect();
        out.extend(values.view([-1]).iter::<f64>().unwrap());
        out
    }

    /// Checkpoint round-trip using safetensors format (`.safetensors` extension).
    #[test]
    fn checkpoint_round_trip_safetensors() {
        let obs_dim = 4_i64;
        let num_actions = 3_i64;
        let obs = Tensor::randn(&[2, obs_dim], (Kind::Float, Device::Cpu));

        let (vs_orig, ac_orig) = make_ac(obs_dim, num_actions);
        let outputs_before = forward_flat(&ac_orig, &obs);

        let path = std::env::temp_dir().join("trolly_gym_checkpoint_test.safetensors");
        save_checkpoint(&vs_orig, &path).expect("save_checkpoint failed");

        let (mut vs_new, ac_new) = make_ac(obs_dim, num_actions);
        load_checkpoint(&mut vs_new, &path).expect("load_checkpoint failed");
        let outputs_after = forward_flat(&ac_new, &obs);

        assert_eq!(outputs_before.len(), outputs_after.len(), "output length mismatch");
        for (i, (a, b)) in outputs_before.iter().zip(outputs_after.iter()).enumerate() {
            assert!(
                (a - b).abs() < 1e-6,
                "forward pass mismatch at index {i}: {a} != {b}"
            );
        }

        let _ = std::fs::remove_file(&path);
    }

    /// Checkpoint round-trip using libtorch C++ module format (`.ckpt` extension).
    /// Confirms `.ckpt` triggers the libtorch C++ module path for both save and load.
    #[test]
    fn checkpoint_round_trip_ckpt() {
        let obs_dim = 4_i64;
        let num_actions = 3_i64;
        let obs = Tensor::randn(&[2, obs_dim], (Kind::Float, Device::Cpu));

        let (vs_orig, ac_orig) = make_ac(obs_dim, num_actions);
        let outputs_before = forward_flat(&ac_orig, &obs);

        let path = std::env::temp_dir().join("trolly_gym_checkpoint_test.ckpt");
        save_checkpoint(&vs_orig, &path).expect("save_checkpoint failed");

        let (mut vs_new, ac_new) = make_ac(obs_dim, num_actions);
        load_checkpoint(&mut vs_new, &path).expect("load_checkpoint failed");
        let outputs_after = forward_flat(&ac_new, &obs);

        assert_eq!(outputs_before.len(), outputs_after.len(), "output length mismatch");
        for (i, (a, b)) in outputs_before.iter().zip(outputs_after.iter()).enumerate() {
            assert!(
                (a - b).abs() < 1e-6,
                "forward pass mismatch at index {i}: {a} != {b}"
            );
        }

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn load_nonexistent_file_returns_error() {
        let (mut vs, _ac) = make_ac(4, 3);
        let result = load_checkpoint(&mut vs, "/tmp/__trolly_gym_nonexistent_12345.safetensors");
        assert!(result.is_err(), "expected error for missing file");
    }
}
