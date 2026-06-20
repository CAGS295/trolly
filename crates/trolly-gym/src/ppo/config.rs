//! Hyperparameter structs for PPO and WoLF-PPO.

/// PPO hyperparameters.
///
/// Defaults match a small-MLP regime suitable for matrix-game experiments:
/// hidden [20, 20], SGD, ε = 0.2, c1 = 0.5, c2 = 0.01.
#[derive(Debug, Clone)]
pub struct PpoConfig {
    /// Clipping parameter ε — bounds the policy ratio in L^CLIP (PPO Eq. 1).
    pub clip_epsilon: f64,
    /// Entropy bonus coefficient c2. Higher values encourage exploration.
    pub entropy_coef: f64,
    /// Value loss coefficient c1.
    pub value_coef: f64,
    /// Gradient update epochs per rollout batch.
    pub ppo_epochs: usize,
    /// Hidden layer sizes for the shared actor-critic MLP.
    /// Default `[20, 20]` per Ratcliffe et al. matrix-game experiments.
    pub hidden_sizes: Vec<i64>,
    /// Base learning rate (PPO only; WoLF-PPO overrides with dual rates).
    pub lr: f64,
    /// Use Adam optimizer when true; SGD is the default.
    pub use_adam: bool,
}

impl Default for PpoConfig {
    fn default() -> Self {
        Self {
            clip_epsilon: 0.2,
            entropy_coef: 0.01,
            value_coef: 0.5,
            ppo_epochs: 4,
            hidden_sizes: vec![20, 20],
            lr: 0.01,
            use_adam: false,
        }
    }
}

/// WoLF-PPO hyperparameters (Ratcliffe et al., IEEE CoG 2019, paper_176).
///
/// WoLF (Win-or-Learn-Fast) extends PPO with dual learning rates selected by
/// comparing the current episode payoff against a rolling average estimate of
/// the Nash-Equilibrium Seeking (NES) payoff:
///
/// - **α_WIN**: used when `current_payoff > rolling_avg` (winning regime).
///   Smaller rate to converge at equilibrium.
/// - **α_LOSE**: used otherwise (losing regime).
///   Larger rate to adapt and escape suboptimal strategies.
///
/// Ratio constraint: `α_WIN = α_LOSE / 4`.
#[derive(Debug, Clone)]
pub struct WolfPpoConfig {
    /// Base PPO config (shared MLP architecture and loss coefficients).
    pub ppo: PpoConfig,
    /// Learning rate when losing (current payoff ≤ rolling estimate).
    pub alpha_lose: f64,
    /// Learning rate when winning (current payoff > rolling estimate).
    /// Must satisfy `alpha_win = alpha_lose / 4`.
    pub alpha_win: f64,
    /// Rolling window length for the NES payoff estimate.
    pub payoff_window: usize,
}

impl Default for WolfPpoConfig {
    fn default() -> Self {
        let alpha_lose = 0.01;
        Self {
            ppo: PpoConfig::default(),
            alpha_lose,
            alpha_win: alpha_lose / 4.0,
            payoff_window: 100,
        }
    }
}

impl WolfPpoConfig {
    /// Set `alpha_lose`; derives `alpha_win = alpha_lose / 4` automatically.
    pub fn with_alpha_lose(mut self, alpha_lose: f64) -> Self {
        self.alpha_lose = alpha_lose;
        self.alpha_win = alpha_lose / 4.0;
        self
    }
}
