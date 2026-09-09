//! PPO and WoLF-PPO actor-critic training primitives (requires `--features torch`).
//!
//! # Modules
//!
//! - [`config`] — [`PpoConfig`] and [`WolfPpoConfig`] with sensible defaults.
//! - [`actor_critic`] — [`ActorCritic`] model selector: MLP or liquid backend.
//! - [`lnn_actor_critic`] — [`LiquidActorCritic`] liquid neural network backend.
//! - [`gaussian_actor_critic`] — tanh-Gaussian MLP for WP-033 ladder inventory.
//! - [`rung_liquid`] — WP-034 Liquid-on-rungs Gaussian FA.
//! - [`ppo`] — [`PpoTrainer`] with clipped surrogate objective.
//! - [`wolf_ppo`] — [`WolfPpoTrainer`] with dual WoLF learning-rate selection.

pub mod actor_critic;
pub mod config;
pub mod gaussian_actor_critic;
pub mod lnn_actor_critic;
pub mod rung_liquid;
pub mod ppo;
pub mod wolf_ppo;

pub use actor_critic::ActorCritic;
pub use config::{ActorCriticArchitecture, PpoConfig, WolfPpoConfig};
pub use gaussian_actor_critic::GaussianActorCritic;
pub use lnn_actor_critic::LiquidActorCritic;
pub use rung_liquid::RungLiquidGaussian;
pub use ppo::{PpoTrainer, RolloutBatch};
pub use wolf_ppo::WolfPpoTrainer;
