//! PPO clipped surrogate, value, and entropy losses (Eq. 1).

use tch::{Kind, Tensor};

use super::config::PpoConfig;

/// Per-term loss values from a PPO update (for logging and tests).
#[derive(Debug, Clone, Copy)]
pub struct PpoLossBreakdown {
    /// Mean clipped surrogate objective L^CLIP (higher is better; we negate for minimization).
    pub clip_loss: f64,
    /// Mean squared value error L^VF.
    pub value_loss: f64,
    /// Mean policy entropy S[π].
    pub entropy: f64,
    /// Scalar minimized by the optimizer (−L^CLIP + c1 L^VF − c2 S).
    pub total: f64,
}

/// Compute PPO losses per Ratcliffe et al. (2019) Eq. 1.
///
/// Maximizes `E[L^CLIP − c1 L^VF + c2 S]`; returns the negated scalar for gradient descent.
pub fn ppo_loss(
    config: &PpoConfig,
    values: &Tensor,
    logits: &Tensor,
    actions: &Tensor,
    old_log_probs: &Tensor,
    returns: &Tensor,
    advantages: &Tensor,
) -> (Tensor, PpoLossBreakdown) {
    let log_probs = logits.log_softmax(-1, Kind::Float);
    let probs = logits.softmax(-1, Kind::Float);
    let index = actions.unsqueeze(-1);
    let new_log_probs = log_probs.gather(-1, &index, false).squeeze_dim(-1);

    let ratio = (new_log_probs - old_log_probs).exp();
    let clipped_ratio = ratio.clamp(1.0 - config.clip_eps, 1.0 + config.clip_eps);
    let surr1 = &ratio * advantages;
    let surr2 = clipped_ratio * advantages;
    let clip_objective = surr1.minimum(&surr2).mean(Kind::Float);
    let value_loss = (values - returns).pow_tensor_scalar(2).mean(Kind::Float);
    let entropy = (-&log_probs * &probs)
        .sum_dim_intlist(-1, false, Kind::Float)
        .mean(Kind::Float);

    let breakdown = PpoLossBreakdown {
        clip_loss: f64::try_from(-&clip_objective).unwrap_or(f64::NAN),
        value_loss: f64::try_from(&value_loss).unwrap_or(f64::NAN),
        entropy: f64::try_from(&entropy).unwrap_or(f64::NAN),
        total: 0.0,
    };

    let total = -&clip_objective + config.value_coef * &value_loss - config.entropy_coef * &entropy;
    let breakdown = PpoLossBreakdown {
        total: f64::try_from(&total).unwrap_or(f64::NAN),
        ..breakdown
    };

    (total, breakdown)
}

/// Returns true when `loss` and all breakdown terms are finite (no NaN/Inf).
pub fn losses_are_finite(loss: &Tensor, breakdown: &PpoLossBreakdown) -> bool {
    loss.isfinite().all().int64_value(&[]) == 1
        && breakdown.clip_loss.is_finite()
        && breakdown.value_loss.is_finite()
        && breakdown.entropy.is_finite()
        && breakdown.total.is_finite()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ppo::config::PpoConfig;
    use tch::{Device, Tensor};

    #[test]
    fn ppo_loss_finite_on_synthetic_batch() {
        let config = PpoConfig::default();
        let batch = 8;
        let device = Device::Cpu;

        let values = Tensor::zeros([batch], (Kind::Float, device));
        let logits = Tensor::zeros([batch, config.num_actions], (Kind::Float, device));
        let actions = Tensor::zeros([batch], (Kind::Int64, device));
        let old_log_probs = Tensor::zeros([batch], (Kind::Float, device));
        let returns = Tensor::ones([batch], (Kind::Float, device));
        let advantages = Tensor::ones([batch], (Kind::Float, device));

        let (loss, breakdown) = ppo_loss(
            &config,
            &values,
            &logits,
            &actions,
            &old_log_probs,
            &returns,
            &advantages,
        );
        assert!(losses_are_finite(&loss, &breakdown));
    }
}
