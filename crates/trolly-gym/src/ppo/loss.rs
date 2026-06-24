//! PPO clipped surrogate, value, and entropy losses (Eq. 1 in WoLF-PPO paper).

use tch::{Kind, Tensor};

/// Clipped surrogate objective L^CLIP (maximize → return negative for minimization).
pub fn clipped_surrogate_loss(
    new_log_probs: &Tensor,
    old_log_probs: &Tensor,
    advantages: &Tensor,
    clip_epsilon: f64,
) -> Tensor {
    let ratio = (new_log_probs - old_log_probs).exp();
    let surr1 = &ratio * advantages;
    let surr2 = ratio.clamp(1.0 - clip_epsilon, 1.0 + clip_epsilon) * advantages;
    -surr1.min_other(&surr2).mean(Kind::Float)
}

/// Squared value error L^VF.
pub fn value_loss(values: &Tensor, returns: &Tensor) -> Tensor {
    (values - returns).pow_tensor_scalar(2).mean(Kind::Float)
}

/// Mean entropy bonus S[π] (maximize → subtract weighted entropy in total loss).
pub fn entropy_bonus(entropy: &Tensor) -> Tensor {
    entropy.mean(Kind::Float)
}

/// Combined PPO loss: L^CLIP − c1·L^VF − c2·S (all terms suitable for `.backward()`).
pub fn ppo_loss(
    new_log_probs: &Tensor,
    old_log_probs: &Tensor,
    advantages: &Tensor,
    values: &Tensor,
    returns: &Tensor,
    entropy: &Tensor,
    clip_epsilon: f64,
    value_coef: f64,
    entropy_coef: f64,
) -> (Tensor, Tensor, Tensor, Tensor) {
    let clip_loss = clipped_surrogate_loss(new_log_probs, old_log_probs, advantages, clip_epsilon);
    let vf_loss = value_loss(values, returns);
    let ent = entropy_bonus(entropy);
    let total = &clip_loss + value_coef * &vf_loss - entropy_coef * &ent;
    (total, clip_loss, vf_loss, ent)
}
