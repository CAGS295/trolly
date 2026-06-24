//! Two-player zero-sum matrix games for WoLF-PPO paper reproduction (WP-019).
//!
//! Provides standard and weighted variants of **Matching Pennies** and
//! **Rock–Paper–Scissors** from Ratcliffe et al. (IEEE CoG 2019), along with a
//! self-play training harness that drives the PPO / WoLF-PPO trainers from
//! `crate::ppo` and measures convergence to the known NES.
//!
//! # Quick start
//!
//! ```ignore
//! use trolly_gym::games::{
//!     matching_pennies::{matching_pennies_weighted, WEIGHTED_NES},
//!     run_wolf_ppo_self_play, SelfPlayConfig,
//! };
//! use trolly_gym::ppo::WolfPpoConfig;
//!
//! let game = matching_pennies_weighted();
//! let result = run_wolf_ppo_self_play(
//!     &game,
//!     &WEIGHTED_NES,
//!     SelfPlayConfig::default(),
//!     WolfPpoConfig::default(),
//! );
//! println!("max distance (last 10 updates): {:.4}", result.max_distance_last_10);
//! ```

pub mod matching_pennies;
pub mod matrix_game;
pub mod metrics;
pub mod rock_paper_scissors;
pub mod trainer;

pub use matching_pennies::{matching_pennies_standard, matching_pennies_weighted};
pub use matrix_game::MatrixGame;
pub use metrics::euclidean_distance_to_nes;
pub use rock_paper_scissors::{rps_standard, rps_weighted};
pub use trainer::{run_ppo_self_play, run_wolf_ppo_self_play, SelfPlayConfig, SelfPlayResult};
