//! WoLF-PPO / PPO training driver, rollout conversion, and checkpoint I/O.
//!
//! Gated behind the crate `torch` feature.

mod checkpoint;
mod collector;
mod driver;
mod env_rollout;

pub use checkpoint::{load_checkpoint, save_checkpoint, CheckpointError};
pub use collector::rollout_collector_to_batch;
pub use driver::{PpoTrainingDriver, TrainConfig, TrainStepMetrics, WolfPpoTrainingDriver};
pub use env_rollout::collect_env_rollout_step;
