//! libtorch.rs integration placeholder (feature `torch`).
//!
//! When the `torch` feature is enabled, this module is the hook point for
//! [`tch`](https://docs.rs/tch) / libtorch-backed policy inference. The default
//! build keeps this as a pure-Rust stub so CI does not require libtorch installed.

/// Stub policy that will wrap a libtorch module checkpoint loader.
#[derive(Debug, Clone, Default)]
pub struct TorchPolicy {
    loaded: bool,
}

impl TorchPolicy {
    pub fn new() -> Self {
        Self { loaded: false }
    }

    /// Mark the policy as ready (placeholder for checkpoint load).
    pub fn load_stub(&mut self) {
        self.loaded = true;
    }

    pub fn is_loaded(&self) -> bool {
        self.loaded
    }

    /// Integration point: run forward pass on an observation vector and return action index.
    ///
    /// Real implementation will convert `observation` to a `tch::Tensor`, invoke the
    /// traced module, and decode logits into a discrete action.
    pub fn infer(&self, observation: &[f64]) -> usize {
        if !self.loaded || observation.is_empty() {
            return 0;
        }
        observation
            .iter()
            .fold(0.0_f64, |acc, v| acc + v)
            .abs() as usize
            % 8
    }
}

// Expected libtorch.rs wiring (not built in default CI):
// 1. Add optional `tch` dependency behind `torch` in `Cargo.toml`.
// 2. Install libtorch (CPU or CUDA) and set `LIBTORCH` / `LD_LIBRARY_PATH`.
// 3. Replace `TorchPolicy::infer` with `tch::CModule::load` + tensor forward.
