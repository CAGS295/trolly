//! Convert on-policy buffers to PPO batches and shared advantage helpers.

use tch::{Device, Kind, Tensor};

use crate::ppo::{compute_advantages, tensor_from_vec_f64, tensor_from_vec_i64, RolloutBatch};
use crate::replay::OnPolicyRolloutBuffer;

/// Discount factor for advantage computation (matrix games use γ=0).
pub const DEFAULT_GAMMA: f64 = 0.0;

/// GAE λ (matrix games use undiscounted Monte Carlo returns).
pub const DEFAULT_GAE_LAMBDA: f64 = 0.0;

/// Build a [`RolloutBatch`] from collected on-policy steps.
pub fn rollout_buffer_to_batch(
    buffer: &OnPolicyRolloutBuffer,
    gamma: f64,
    gae_lambda: f64,
    device: Device,
) -> RolloutBatch {
    let steps = buffer.steps();
    assert!(!steps.is_empty(), "rollout buffer must not be empty");

    let n = steps.len();
    let obs_dim = steps[0].observation.len() as i64;

    let mut obs_flat = Vec::with_capacity(n * obs_dim as usize);
    let mut actions = Vec::with_capacity(n);
    let mut old_log_probs = Vec::with_capacity(n);
    let mut rewards = Vec::with_capacity(n);
    let mut values = Vec::with_capacity(n);

    for step in steps {
        assert_eq!(
            step.observation.len(),
            obs_dim as usize,
            "all rollout observations must share obs_dim"
        );
        obs_flat.extend_from_slice(&step.observation);
        actions.push(step.action);
        old_log_probs.push(step.log_prob);
        rewards.push(step.reward);
        values.push(step.value);
    }

    let mut advantages = compute_advantages(&rewards, &values, gamma, gae_lambda).0;
    normalize_advantages(&mut advantages);
    let returns: Vec<f64> = advantages
        .iter()
        .zip(values.iter())
        .map(|(a, v)| a + v)
        .collect();

    let observations = Tensor::from_slice(&obs_flat)
        .to_kind(Kind::Float)
        .reshape(&[n as i64, obs_dim])
        .to_device(device);

    RolloutBatch::new(
        observations,
        tensor_from_vec_i64(&actions).to_device(device),
        tensor_from_vec_f64(&old_log_probs).to_device(device),
        tensor_from_vec_f64(&advantages).to_device(device),
        tensor_from_vec_f64(&returns).to_device(device),
    )
}

/// Per-dimension standardization of advantages (common PPO practice).
pub fn normalize_advantages(advantages: &mut [f64]) {
    if advantages.is_empty() {
        return;
    }
    let mean = advantages.iter().sum::<f64>() / advantages.len() as f64;
    let var = advantages
        .iter()
        .map(|a| (a - mean).powi(2))
        .sum::<f64>()
        / advantages.len() as f64;
    let std = var.sqrt().max(1e-8);
    for a in advantages {
        *a = (*a - mean) / std;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::replay::OnPolicyStep;
    use tch::Device;

    #[test]
    fn rollout_buffer_to_batch_shapes() {
        let mut buf = OnPolicyRolloutBuffer::new();
        for i in 0..4 {
            buf.push(OnPolicyStep {
                observation: vec![i as f32, 1.0],
                action: i % 2,
                log_prob: -0.3,
                value: 0.0,
                reward: if i % 2 == 0 { 1.0 } else { -1.0 },
                done: false,
            });
        }

        let batch = rollout_buffer_to_batch(&buf, DEFAULT_GAMMA, DEFAULT_GAE_LAMBDA, Device::Cpu);
        assert_eq!(batch.observations.size(), vec![4, 2]);
        assert_eq!(batch.actions.size(), vec![4]);
        assert_eq!(batch.old_log_probs.size(), vec![4]);
        assert_eq!(batch.advantages.size(), vec![4]);
        assert_eq!(batch.returns.size(), vec![4]);
    }
}
