//! Proximal Policy Optimisation (PPO) and Win-or-Learn-Fast PPO (WoLF-PPO).
//!
//! Implements the clipped surrogate objective, value loss, and entropy bonus from
//! [Schulman et al., 2017](https://arxiv.org/abs/1707.06347), plus the WoLF
//! dual learning-rate extension from
//! [Ratcliffe et al., IEEE CoG 2019](https://ieee-cog.org/2019/papers/paper_176.pdf).

mod actor_critic;
mod batch;
mod config;
mod loss;
mod trainer;

pub use actor_critic::{new_actor_critic, ActorCritic, cpu_device};
pub use batch::RolloutBatch;
pub use config::{PpoConfig, WolfPpoConfig};
pub use loss::{clipped_surrogate_loss, entropy_bonus, ppo_loss, value_loss};
pub use trainer::{compute_advantages, PpoTrainer, UpdateMetrics, WolfPpoTrainer};
