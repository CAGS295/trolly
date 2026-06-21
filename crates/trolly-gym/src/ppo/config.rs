//! PPO and WoLF-PPO hyperparameter configuration.

/// Standard PPO hyperparameters (Schulman et al.; WoLF-PPO paper Eq. 1).
#[derive(Debug, Clone, Copy)]
pub struct PpoConfig {
    /// Clipped surrogate ratio bound ε.
    pub clip_epsilon: f64,
    /// Value loss coefficient c1.
    pub value_coef: f64,
    /// Entropy bonus coefficient c2.
    pub entropy_coef: f64,
    /// PPO epochs per rollout batch.
    pub ppo_epochs: usize,
    /// SGD learning rate (used when optimizer is Sgd).
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
            learning_rate: 0.01,
            use_adam: false,
        }
    }
}

/// WoLF-PPO extension: dual learning rates keyed on payoff vs estimated NES payoff.
#[derive(Debug, Clone, Copy)]
pub struct WolfPpoConfig {
    pub ppo: PpoConfig,
    /// α_LOSE — learning rate when current expected payoff ≤ rolling NES estimate.
    pub alpha_lose: f64,
    /// Exponential moving-average decay for the NES payoff estimate (0, 1].
    pub nes_estimate_decay: f64,
}

impl Default for WolfPpoConfig {
    fn default() -> Self {
        Self {
            ppo: PpoConfig::default(),
            alpha_lose: 0.01,
            nes_estimate_decay: 0.99,
        }
    }
}

impl WolfPpoConfig {
    /// α_WIN = α_LOSE / 4 per Ratcliffe et al. (IEEE CoG 2019).
    pub fn alpha_win(&self) -> f64 {
        self.alpha_lose / 4.0
    }
}
