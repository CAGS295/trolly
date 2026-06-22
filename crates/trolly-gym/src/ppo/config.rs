//! Hyperparameter configuration for PPO and WoLF-PPO.

/// Optimizer used during policy updates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OptimizerKind {
    /// Stochastic gradient descent (paper default for matrix-game experiments).
    Sgd,
    /// Adam optimizer (optional; paper notes similar results).
    Adam,
}

/// PPO hyperparameters (Schulman et al. 2017; Ratcliffe et al. 2019 Eq. 1).
#[derive(Debug, Clone)]
pub struct PpoConfig {
    /// Observation dimensionality (flattened feature vector length).
    pub obs_dim: i64,
    /// Number of discrete actions (categorical policy).
    pub num_actions: i64,
    /// Hidden layer widths for the shared MLP trunk (paper matrix games: `[20, 20]`).
    pub hidden_dims: Vec<i64>,
    /// PPO clip range ε for the probability ratio.
    pub clip_eps: f64,
    /// Value-function coefficient c1 in Eq. 1.
    pub value_coef: f64,
    /// Entropy bonus coefficient c2 in Eq. 1.
    pub entropy_coef: f64,
    /// Number of SGD/Adam epochs per rollout batch.
    pub ppo_epochs: usize,
    /// Base learning rate (α for PPO; α_LOSE baseline for WoLF-PPO).
    pub learning_rate: f64,
    /// Optimizer selection.
    pub optimizer: OptimizerKind,
}

impl Default for PpoConfig {
    fn default() -> Self {
        Self {
            obs_dim: 4,
            num_actions: 2,
            hidden_dims: vec![20, 20],
            clip_eps: 0.2,
            value_coef: 0.5,
            entropy_coef: 0.01,
            ppo_epochs: 4,
            learning_rate: 0.01,
            optimizer: OptimizerKind::Sgd,
        }
    }
}

impl PpoConfig {
    /// Builder-style override for observation dimension.
    pub fn with_obs_dim(mut self, obs_dim: i64) -> Self {
        self.obs_dim = obs_dim;
        self
    }

    /// Builder-style override for action count.
    pub fn with_num_actions(mut self, num_actions: i64) -> Self {
        self.num_actions = num_actions;
        self
    }
}

/// WoLF-PPO extension: dual learning rates keyed off estimated NES payoff.
#[derive(Debug, Clone)]
pub struct WolfPpoConfig {
    /// Shared PPO settings (learning_rate is α_LOSE).
    pub ppo: PpoConfig,
    /// Losing-side learning rate α_LOSE (winning rate α_WIN = α_LOSE / win_lose_ratio).
    pub alpha_lose: f64,
    /// Ratio α_LOSE / α_WIN (paper default: 4).
    pub win_lose_ratio: f64,
}

impl Default for WolfPpoConfig {
    fn default() -> Self {
        let mut ppo = PpoConfig::default();
        ppo.learning_rate = 0.01;
        Self {
            ppo,
            alpha_lose: 0.01,
            win_lose_ratio: 4.0,
        }
    }
}

impl WolfPpoConfig {
    /// α_WIN = α_LOSE / win_lose_ratio per Ratcliffe et al. (2019).
    pub fn alpha_win(&self) -> f64 {
        self.alpha_lose / self.win_lose_ratio
    }

    /// Select learning rate from current batch payoff vs rolling NES estimate.
    pub fn select_learning_rate(&self, current_payoff: f64, estimated_nes_payoff: f64) -> f64 {
        if current_payoff > estimated_nes_payoff {
            self.alpha_win()
        } else {
            self.alpha_lose
        }
    }
}
