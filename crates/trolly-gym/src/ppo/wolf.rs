//! WoLF learning-rate selection and rolling average NES payoff estimate.

use super::config::WolfPpoConfig;

/// Tracks the running average payoff used as an estimated NES payoff (Ratcliffe et al.).
#[derive(Debug, Clone, Copy, Default)]
pub struct NesPayoffEstimate {
    average_payoff: f64,
    update_count: u64,
}

impl NesPayoffEstimate {
    /// Current rolling average payoff estimate.
    pub fn average(&self) -> f64 {
        self.average_payoff
    }

    /// Number of payoff observations incorporated.
    pub fn count(&self) -> u64 {
        self.update_count
    }

    /// Incorporate a new batch mean reward into the running average.
    pub fn observe(&mut self, batch_mean_reward: f64) {
        self.update_count += 1;
        let n = self.update_count as f64;
        self.average_payoff += (batch_mean_reward - self.average_payoff) / n;
    }

    /// Select α_WIN or α_LOSE from current vs estimated payoff.
    pub fn select_learning_rate(
        &self,
        config: &WolfPpoConfig,
        current_payoff: f64,
    ) -> WolfLearningRate {
        let lr = config.select_learning_rate(current_payoff, self.average_payoff);
        let mode = if current_payoff > self.average_payoff {
            WolfMode::Win
        } else {
            WolfMode::Lose
        };
        WolfLearningRate { lr, mode }
    }
}

/// Which WoLF branch was taken for the active learning rate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WolfMode {
    /// Current payoff exceeds estimate — cautious α_WIN.
    Win,
    /// Current payoff at or below estimate — fast α_LOSE.
    Lose,
}

/// Active WoLF learning rate for one policy update.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WolfLearningRate {
    pub lr: f64,
    pub mode: WolfMode,
}
