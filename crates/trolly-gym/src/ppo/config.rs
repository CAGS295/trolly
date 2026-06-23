//! Hyperparameter configuration for PPO and WoLF-PPO.

/// Optimizer used during policy updates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OptimizerKind {
    /// Stochastic gradient descent (paper default).
    Sgd,
    /// Adam — optional; adaptive moments can interact with WoLF learning-rate switching.
    Adam,
}

/// Standard PPO hyperparameters (Schulman et al.; Ratcliffe et al. Eq. 1).
#[derive(Debug, Clone)]
pub struct PpoConfig {
    /// Clipped surrogate ratio bound ε.
    pub clip_epsilon: f64,
    /// Value-function loss coefficient c1.
    pub value_coef: f64,
    /// Entropy bonus coefficient c2.
    pub entropy_coef: f64,
    /// Number of epochs over each rollout batch.
    pub ppo_epochs: usize,
    /// Base learning rate for the policy/value network.
    pub learning_rate: f64,
    /// Hidden layer widths for the actor–critic MLP.
    pub hidden_layers: Vec<i64>,
    /// Optimizer choice (SGD default per WoLF-PPO paper).
    pub optimizer: OptimizerKind,
}

impl Default for PpoConfig {
    fn default() -> Self {
        Self {
            clip_epsilon: 0.2,
            value_coef: 0.5,
            entropy_coef: 0.01,
            ppo_epochs: 4,
            learning_rate: 0.01,
            hidden_layers: vec![20, 20],
            optimizer: OptimizerKind::Sgd,
        }
    }
}

/// WoLF-PPO extension: dual learning rates keyed off estimated NES payoff.
#[derive(Debug, Clone)]
pub struct WolfPpoConfig {
    /// Base PPO settings shared with vanilla PPO.
    pub ppo: PpoConfig,
    /// Learning rate when current expected payoff is below the rolling estimate (lose fast).
    pub alpha_lose: f64,
    /// Ratio between α_LOSE and α_WIN (paper uses 4: α_WIN = α_LOSE / win_lose_ratio).
    pub win_lose_ratio: f64,
}

impl Default for WolfPpoConfig {
    fn default() -> Self {
        Self {
            ppo: PpoConfig::default(),
            alpha_lose: 0.1,
            win_lose_ratio: 4.0,
        }
    }
}

impl WolfPpoConfig {
    /// Learning rate when current payoff exceeds the rolling NES payoff estimate (win slow).
    pub fn alpha_win(&self) -> f64 {
        self.alpha_lose / self.win_lose_ratio
    }

    /// Select WoLF learning rate from current vs estimated payoff.
    pub fn select_learning_rate(&self, current_payoff: f64, estimated_nes_payoff: f64) -> f64 {
        if current_payoff > estimated_nes_payoff {
            self.alpha_win()
        } else {
            self.alpha_lose
        }
    }
}
