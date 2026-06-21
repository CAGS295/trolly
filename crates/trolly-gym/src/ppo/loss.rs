//! PPO clipped surrogate, value, and entropy losses (Eq. 1).

use tch::{Kind, Tensor};

use super::config::PpoConfig;

#[derive(Debug, Clone, Copy, Default)]
pub struct PpoLossBreakdown {
    pub policy_loss: f64,
    pub value_loss: f64,
    pub entropy: f64,
    pub total: f64,
}

impl PpoLossBreakdown {
    pub fn accumulate(&mut self, other: &Self) {
        self.policy_loss += other.policy_loss;
        self.value_loss += other.value_loss;
        self.entropy += other.entropy;
        self.total += other.total;
    }

    pub fn scale(&mut self, factor: f64) {
        self.policy_loss *= factor;
        self.value_loss *= factor;
        self.entropy *= factor;
        self.total *= factor;
    }
}

pub struct PpoLossTensors {
    pub breakdown: PpoLossBreakdown,
    pub total: Tensor,
}

/// Compute PPO losses; `total` tensor is ready for `backward()`.
pub fn ppo_losses(
    log_probs: &Tensor,
    old_log_probs: &Tensor,
    advantages: &Tensor,
    values: &Tensor,
    returns: &Tensor,
    entropy: &Tensor,
    config: &PpoConfig,
) -> PpoLossTensors {
    let ratio = (log_probs - old_log_probs).exp();
    let surr1 = &ratio * advantages;
    let surr2 = ratio
        .clamp(1.0 - config.clip_epsilon, 1.0 + config.clip_epsilon)
        * advantages;
    let policy_loss = -surr1.min_other(&surr2).mean(Kind::Float);

    let value_loss = (values - returns)
        .pow_tensor_scalar(2)
        .mean(Kind::Float);

    let entropy_mean = entropy.mean(Kind::Float);
    let total = policy_loss.shallow_clone() * 1.0
        + value_loss.shallow_clone() * config.value_coef
        - entropy_mean.shallow_clone() * config.entropy_coef;

    PpoLossTensors {
        breakdown: PpoLossBreakdown {
            policy_loss: policy_loss.double_value(&[]),
            value_loss: value_loss.double_value(&[]),
            entropy: entropy_mean.double_value(&[]),
            total: total.double_value(&[]),
        },
        total,
    }
}
