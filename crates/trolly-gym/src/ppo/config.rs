//! Hyperparameter configuration for PPO and WoLF-PPO.

/// Standard PPO hyperparameters (Schulman et al., 2017).
#[derive(Debug, Clone)]
pub struct PpoConfig {
    /// Clipped surrogate ratio bound ε.
    pub clip_epsilon: f64,
    /// Value loss coefficient c1.
    pub value_coef: f64,
    /// Entropy bonus coefficient c2.
    pub entropy_coef: f64,
    /// PPO epochs per rollout batch.
    pub ppo_epochs: usize,
    /// Hidden layer widths for the actor–critic MLP (paper default `[20, 20]`).
    pub hidden_layers: Vec<i64>,
    /// Base learning rate (SGD default; Adam uses the same field when enabled).
    pub learning_rate: f64,
    /// Use Adam instead of SGD when true.
    pub use_adam: bool,
}

impl Default for PpoConfig {
    fn default() -> Self {
        Self {
            clip_epsilon: 0.2,
            value_coef: 0.5,
            entropy_coef: 0.01,
            ppo_epochs: 4,
            hidden_layers: vec![20, 20],
            learning_rate: 0.01,
            use_adam: false,
        }
    }
}

/// WoLF-PPO extension: dual learning rates per Ratcliffe et al. (IEEE CoG 2019).
#[derive(Debug, Clone)]
pub struct WolfPpoConfig {
    pub ppo: PpoConfig,
    /// Losing learning rate α_LOSE; α_WIN = α_LOSE / 4.
    pub alpha_lose: f64,
    /// Exponential moving-average decay for estimated NES payoff (0 = arithmetic mean).
    pub payoff_ema_decay: f64,
}

impl Default for WolfPpoConfig {
    fn default() -> Self {
        Self {
            ppo: PpoConfig::default(),
            alpha_lose: 0.01,
            payoff_ema_decay: 0.99,
        }
    }
}

impl WolfPpoConfig {
    /// Winning learning rate α_WIN = α_LOSE / 4 (paper ratio).
    pub fn alpha_win(&self) -> f64 {
        self.alpha_lose / 4.0
    }
}
