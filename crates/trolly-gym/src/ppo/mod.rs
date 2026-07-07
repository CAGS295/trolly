//! PPO and WoLF-PPO actor-critic training primitives (requires `--features torch`).
//!
//! # Modules
//!
//! - [`config`] — [`PpoConfig`] and [`WolfPpoConfig`] with sensible defaults.
//! - [`actor_critic`] — [`ActorCritic`] model selector: MLP or liquid backend.
//! - [`lnn_actor_critic`] — [`LiquidActorCritic`] liquid neural network backend.
//! - [`ppo`] — [`PpoTrainer`] with clipped surrogate objective.
//! - [`wolf_ppo`] — [`WolfPpoTrainer`] with dual WoLF learning-rate selection.

pub mod actor_critic;
pub mod config;
pub mod lnn_actor_critic;
pub mod ppo;
pub mod wolf_ppo;

pub use actor_critic::ActorCritic;
pub use config::{ActorCriticArchitecture, PpoConfig, WolfPpoConfig};
pub use lnn_actor_critic::LiquidActorCritic;
pub use ppo::{PpoTrainer, RolloutBatch};
pub use wolf_ppo::WolfPpoTrainer;
