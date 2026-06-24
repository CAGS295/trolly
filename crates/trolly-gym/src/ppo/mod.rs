//! PPO and WoLF-PPO actor-critic training primitives (requires `--features torch`).
//!
//! # Modules
//!
//! - [`config`] — [`PpoConfig`] and [`WolfPpoConfig`] with sensible defaults.
//! - [`actor_critic`] — [`ActorCritic`] MLP: categorical policy + value head.
//! - [`ppo`] — [`PpoTrainer`] with clipped surrogate objective.
//! - [`wolf_ppo`] — [`WolfPpoTrainer`] with dual WoLF learning-rate selection.

pub mod actor_critic;
pub mod config;
pub mod ppo;
pub mod wolf_ppo;

pub use actor_critic::ActorCritic;
pub use config::{PpoConfig, WolfPpoConfig};
pub use ppo::{PpoTrainer, RolloutBatch};
pub use wolf_ppo::WolfPpoTrainer;
