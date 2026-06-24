//! WoLF-PPO training loop, rollout collection, and checkpoint I/O.
//!
//! All items require `--features torch` (gated in `lib.rs`).
//!
//! # Sub-modules
//!
//! - [`rollout`] — [`OnPolicyTransition`], [`RolloutCollector`], GAE computation.
//! - [`driver`] — [`WolfPpoTrainDriver`], [`TrainMetrics`], [`TrainDriverConfig`].
//! - [`checkpoint`] — [`save_checkpoint`] / [`load_checkpoint`] for actor-critic weights.
//!
//! # Quick-start example
//!
//! ```rust,ignore
//! use trolly_gym::train::{
//!     WolfPpoTrainDriver, TrainDriverConfig,
//!     checkpoint::{save_checkpoint, load_checkpoint},
//! };
//! use trolly_gym::ppo::WolfPpoConfig;
//!
//! let cfg = TrainDriverConfig { obs_dim: 4, num_actions: 3, horizon: 64, ..Default::default() };
//! let mut driver = WolfPpoTrainDriver::new(cfg, WolfPpoConfig::default());
//!
//! for _i in 0..100 {
//!     let metrics = driver.train_step(
//!         vec![0.0; 4],
//!         |obs, _action| trolly_gym::train::rollout::StepOutput { next_observation: obs, reward: 0.0, done: false },
//!         0.0,
//!         None,
//!     );
//!     println!("policy_loss={:.4} entropy={:.4} lr={}", metrics.policy_loss, metrics.entropy, metrics.active_lr);
//! }
//!
//! save_checkpoint(&driver.trainer.inner.vs, "/tmp/model.safetensors").unwrap();
//! ```

pub mod checkpoint;
pub mod driver;
pub mod rollout;

pub use checkpoint::{load_checkpoint, save_checkpoint};
pub use driver::{TrainDriverConfig, TrainMetrics, WolfPpoTrainDriver};
pub use rollout::{OnPolicyTransition, RolloutCollector, StepOutput, compute_gae};
