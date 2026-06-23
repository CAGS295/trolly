//! PPO clipped surrogate, value, and entropy terms (Ratcliffe et al. Eq. 1).

use tch::{Kind, Tensor};

fn scalar_f64(t: &Tensor) -> f64 {
    f64::try_from(t).expect("scalar tensor")
}

/// Scalar breakdown of the PPO objective components.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PpoLossBreakdown {
    pub policy_loss: f64,
    pub value_loss: f64,
    pub entropy: f64,
    pub total_loss: f64,
}

/// Compute PPO losses to **minimize** (negative of the maximization objective).
///
/// Returns `(total_loss_tensor, breakdown)` where the total combines
/// `-L^CLIP + c1 * L^VF - c2 * S`.
pub fn ppo_loss(
    log_probs: &Tensor,
    old_log_probs: &Tensor,
    advantages: &Tensor,
    values: &Tensor,
    returns: &Tensor,
    entropy: &Tensor,
    clip_epsilon: f64,
    value_coef: f64,
    entropy_coef: f64,
) -> (Tensor, PpoLossBreakdown) {
    let ratio = (log_probs - old_log_probs).exp();
    let clipped_ratio = ratio.clamp(1.0 - clip_epsilon, 1.0 + clip_epsilon);
    let surr1 = &ratio * advantages;
    let surr2 = clipped_ratio * advantages;
    let policy_loss = -surr1.min_other(&surr2).mean(Kind::Float);

    let value_loss = (values - returns).pow_tensor_scalar(2).mean(Kind::Float);

    let mean_entropy = entropy.mean(Kind::Float);
    let total = policy_loss.shallow_clone() + value_coef * value_loss.shallow_clone()
        - entropy_coef * mean_entropy.shallow_clone();

    let breakdown = PpoLossBreakdown {
        policy_loss: scalar_f64(&policy_loss),
        value_loss: scalar_f64(&value_loss),
        entropy: scalar_f64(&mean_entropy),
        total_loss: scalar_f64(&total),
    };

    (total, breakdown)
}

/// Returns `true` when every scalar component is finite (no NaN/Inf).
pub fn is_finite_breakdown(b: &PpoLossBreakdown) -> bool {
    b.policy_loss.is_finite()
        && b.value_loss.is_finite()
        && b.entropy.is_finite()
        && b.total_loss.is_finite()
}
