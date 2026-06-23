//! On-policy rollout collection and conversion to [`RolloutBatch`](crate::ppo::RolloutBatch).

use tch::{kind::Kind, Device, Tensor};

use crate::action::Action;
use crate::ppo::{ActorCritic, RolloutBatch};
use crate::replay::{OnPolicyRolloutBuffer, OnPolicyTransition};

/// Map discrete gym actions to policy head indices (Hold=0, Buy=1, Sell=2).
pub fn action_to_index(action: Action) -> i64 {
    match action {
        Action::Hold => 0,
        Action::Buy => 1,
        Action::Sell => 2,
    }
}

/// Map policy head index back to a gym action.
pub fn action_from_index(index: i64) -> Action {
    match index {
        0 => Action::Hold,
        1 => Action::Buy,
        _ => Action::Sell,
    }
}

/// Collects on-policy transitions and builds PPO-ready tensor batches.
#[derive(Debug, Default)]
pub struct RolloutCollector {
    buffer: OnPolicyRolloutBuffer,
}

impl RolloutCollector {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            buffer: OnPolicyRolloutBuffer::with_capacity(capacity),
        }
    }

    pub fn len(&self) -> usize {
        self.buffer.len()
    }

    pub fn is_empty(&self) -> bool {
        self.buffer.is_empty()
    }

    pub fn clear(&mut self) {
        self.buffer.clear();
    }

    pub fn buffer(&self) -> &OnPolicyRolloutBuffer {
        &self.buffer
    }

    pub fn buffer_mut(&mut self) -> &mut OnPolicyRolloutBuffer {
        &mut self.buffer
    }

    pub fn push(&mut self, step: OnPolicyTransition) {
        self.buffer.push(step);
    }

    /// Sample an action from the policy, returning `(action, log_prob, value)`.
    pub fn sample_action(ac: &ActorCritic, observation: &[f32]) -> (Action, f32, f32) {
        tch::no_grad(|| {
            let obs = obs_tensor(observation);
            let (logits, values) = ac.forward(&obs);
            let log_probs_all = logits.log_softmax(-1, Kind::Float);
            let probs = logits.softmax(-1, Kind::Float);
            let action_idx = probs.multinomial(1, true).int64_value(&[0]);
            let log_prob = log_probs_all.double_value(&[0, action_idx]);
            let value = values.double_value(&[0, 0]);
            (action_from_index(action_idx), log_prob as f32, value as f32)
        })
    }

    /// Record one env step into the on-policy buffer.
    pub fn record_step(
        &mut self,
        observation: Vec<f32>,
        action: Action,
        log_prob: f32,
        value: f32,
        reward: f32,
        done: bool,
    ) {
        self.push(OnPolicyTransition {
            observation,
            action,
            log_prob,
            value,
            reward,
            done,
        });
    }

    /// Convert collected steps into a [`RolloutBatch`] for `policy_update`.
    pub fn finish(&self) -> RolloutBatch {
        finish_rollout_batch(&self.buffer)
    }
}

/// Build a [`RolloutBatch`] from an on-policy buffer (advantages = reward − value).
pub fn finish_rollout_batch(buffer: &OnPolicyRolloutBuffer) -> RolloutBatch {
    assert!(
        !buffer.is_empty(),
        "on-policy rollout buffer must contain at least one step"
    );

    let batch_size = buffer.len() as i64;
    let obs_dim = buffer.steps()[0].observation.len() as i64;

    let mut obs_flat = Vec::with_capacity(buffer.len() * obs_dim as usize);
    let mut actions = Vec::with_capacity(buffer.len());
    let mut old_log_probs = Vec::with_capacity(buffer.len());
    let mut values = Vec::with_capacity(buffer.len());
    let mut rewards = Vec::with_capacity(buffer.len());

    for step in buffer.steps() {
        obs_flat.extend_from_slice(&step.observation);
        actions.push(action_to_index(step.action));
        old_log_probs.push(step.log_prob as f64);
        values.push(step.value as f64);
        rewards.push(step.reward as f64);
    }

    let obs = Tensor::from_slice(&obs_flat)
        .to_kind(Kind::Float)
        .to_device(Device::Cpu)
        .view([batch_size, obs_dim]);
    let actions = Tensor::from_slice(&actions).to_device(Device::Cpu);
    let old_log_probs = Tensor::from_slice(&old_log_probs)
        .to_kind(Kind::Float)
        .to_device(Device::Cpu);
    let values_t = Tensor::from_slice(&values)
        .to_kind(Kind::Float)
        .to_device(Device::Cpu);
    let returns = Tensor::from_slice(&rewards)
        .to_kind(Kind::Float)
        .to_device(Device::Cpu);
    let advantages = &returns - values_t.detach();
    let mean_reward = rewards.iter().sum::<f64>() / rewards.len() as f64;

    RolloutBatch {
        obs,
        actions,
        old_log_probs,
        advantages,
        returns,
        mean_reward,
    }
}

fn obs_tensor(observation: &[f32]) -> Tensor {
    Tensor::from_slice(observation)
        .to_kind(Kind::Float)
        .to_device(Device::Cpu)
        .view([1, observation.len() as i64])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ppo::{PpoConfig, WolfPpoConfig, WolfPpoTrainer};

    #[test]
    fn finish_rollout_batch_shapes_match_steps() {
        let mut collector = RolloutCollector::with_capacity(4);
        collector.record_step(vec![1.0, 2.0], Action::Buy, -0.5, 0.1, 1.0, false);
        collector.record_step(vec![3.0, 4.0], Action::Hold, -0.3, 0.2, -0.5, true);

        let batch = collector.finish();
        assert_eq!(batch.batch_size(), 2);
        assert_eq!(batch.obs.size(), vec![2, 2]);
        assert_eq!(batch.actions.size(), vec![2]);
        assert!((batch.mean_reward - 0.25).abs() < 1e-6);
    }

    #[test]
    fn sample_action_indices_are_valid() {
        let trainer = WolfPpoTrainer::new(4, 3, WolfPpoConfig::default());
        let (action, log_prob, value) =
            RolloutCollector::sample_action(trainer.actor_critic(), &[0.0; 4]);
        assert!(log_prob <= 0.0);
        assert!(matches!(action, Action::Hold | Action::Buy | Action::Sell));
        assert!(value.is_finite());
        let _ = PpoConfig::default();
    }
}
