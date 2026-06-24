//! Scalar training metrics logged per policy update.

use crate::ppo::PpoLossBreakdown;

/// Metrics from one PPO / WoLF-PPO update step.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TrainStepMetrics {
    pub policy_loss: f64,
    pub value_loss: f64,
    pub entropy: f64,
    pub total_loss: f64,
    /// Euclidean distance from a reference NES policy when provided.
    pub nes_distance: Option<f64>,
    /// WoLF learning rate applied in this update (`None` for plain PPO).
    pub learning_rate: Option<f64>,
}

impl TrainStepMetrics {
    pub fn from_losses(
        losses: &PpoLossBreakdown,
        nes_distance: Option<f64>,
        learning_rate: Option<f64>,
    ) -> Self {
        Self {
            policy_loss: losses.policy_loss,
            value_loss: losses.value_loss,
            entropy: losses.entropy,
            total_loss: losses.total,
            nes_distance,
            learning_rate,
        }
    }
}

/// Append-only log of training metrics across updates.
#[derive(Debug, Clone, Default)]
pub struct TrainMetricsLog {
    pub steps: Vec<TrainStepMetrics>,
}

impl TrainMetricsLog {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn record(&mut self, metrics: TrainStepMetrics) {
        self.steps.push(metrics);
    }

    pub fn len(&self) -> usize {
        self.steps.len()
    }

    pub fn is_empty(&self) -> bool {
        self.steps.is_empty()
    }

    pub fn last(&self) -> Option<&TrainStepMetrics> {
        self.steps.last()
    }
}
