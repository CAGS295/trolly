//! On-policy rollout collection and conversion to [`RolloutBatch`].
//!
//! # Overview
//!
//! [`RolloutCollector`] steps a generic environment closure, records
//! [`OnPolicyTransition`]s, computes discounted returns and GAE advantages,
//! and converts the result to a [`RolloutBatch`] compatible with
//! [`PpoTrainer`] / [`WolfPpoTrainer`].
//!
//! # Env hook
//!
//! Pass any closure `FnMut(Tensor) -> StepOutput` as the env stepper.  In
//! tests a mock closure is used; in production the closure wraps
//! [`crate::env::Env::step`] and reads the observation from the current
//! [`crate::observation::ObservationWindow`].

use tch::{Device, Kind, Tensor};

use crate::ppo::{ActorCritic, RolloutBatch};

/// One on-policy transition (obs, action, log_prob, value, reward, done).
///
/// Fields match the quantities needed by the PPO surrogate objective and GAE.
#[derive(Debug, Clone)]
pub struct OnPolicyTransition {
    /// Flattened observation at time t.
    pub observation: Vec<f32>,
    /// Discrete action index sampled from the policy.
    pub action: i64,
    /// Log-probability of `action` under the behaviour policy.
    pub log_prob: f32,
    /// Value estimate V(s_t) from the critic.
    pub value: f32,
    /// Scalar reward received after taking `action`.
    pub reward: f32,
    /// Whether this transition ended an episode.
    pub done: bool,
}

/// Lightweight output returned by the env stepper closure.
pub struct StepOutput {
    /// Next observation (used as next state for bootstrapping).
    pub next_observation: Vec<f32>,
    pub reward: f32,
    pub done: bool,
}

/// On-policy rollout collector.
///
/// Collects `horizon` transitions by running a caller-supplied env closure,
/// then exposes [`RolloutCollector::into_batch`] to compute returns /
/// advantages and wrap everything into a [`RolloutBatch`].
pub struct RolloutCollector {
    transitions: Vec<OnPolicyTransition>,
    horizon: usize,
    gamma: f64,
    gae_lambda: f64,
}

impl RolloutCollector {
    /// Create a new collector.
    ///
    /// - `horizon`: number of environment steps per rollout.
    /// - `gamma`: discount factor (e.g. 0.99).
    /// - `gae_lambda`: GAE λ (e.g. 0.95; use 1.0 for plain Monte-Carlo).
    pub fn new(horizon: usize, gamma: f64, gae_lambda: f64) -> Self {
        Self {
            transitions: Vec::with_capacity(horizon),
            horizon,
            gamma,
            gae_lambda,
        }
    }

    /// Collect one rollout by calling `env_step` up to `horizon` times.
    ///
    /// `env_step` receives the current observation and must return a
    /// [`StepOutput`].  The caller is responsible for resetting the
    /// environment when `done` is true.
    ///
    /// `actor_critic` is used to sample actions and estimate values.
    ///
    /// `initial_obs` is the observation at the start of the rollout.
    pub fn collect<F>(
        &mut self,
        actor_critic: &ActorCritic,
        initial_obs: Vec<f32>,
        mut env_step: F,
    ) where
        F: FnMut(Vec<f32>, i64) -> StepOutput,
    {
        self.transitions.clear();
        let obs_dim = initial_obs.len() as i64;
        let mut current_obs = initial_obs;

        for _ in 0..self.horizon {
            let obs_t = Tensor::from_slice(&current_obs)
                .unsqueeze(0)
                .to_kind(Kind::Float);

            let (action, log_prob) = {
                let _no_grad = tch::no_grad_guard();
                actor_critic.action_and_log_prob(&obs_t)
            };
            let (_, value_t) = {
                let _no_grad = tch::no_grad_guard();
                actor_critic.forward(&obs_t)
            };

            let action_i = action.int64_value(&[]);
            let log_prob_f = log_prob.double_value(&[]) as f32;
            let value_f = value_t.double_value(&[0]) as f32;

            let out = env_step(current_obs.clone(), action_i);
            let reward = out.reward;
            let done = out.done;

            self.transitions.push(OnPolicyTransition {
                observation: current_obs,
                action: action_i,
                log_prob: log_prob_f,
                value: value_f,
                reward,
                done,
            });

            current_obs = out.next_observation;
            if done {
                current_obs = vec![0.0_f32; obs_dim as usize];
            }
        }
    }

    /// Convert accumulated transitions into a [`RolloutBatch`] using GAE.
    ///
    /// `bootstrap_value`: critic estimate V(s_{T+1}) for the state after the
    /// last transition (0.0 if the last step was terminal).
    pub fn into_batch(&self, bootstrap_value: f32) -> RolloutBatch {
        let t = self.transitions.len();
        assert!(t > 0, "RolloutCollector: no transitions collected");

        let (returns, advantages) =
            compute_gae(&self.transitions, bootstrap_value, self.gamma, self.gae_lambda);

        let obs_dim = self.transitions[0].observation.len() as i64;
        let obs_flat: Vec<f32> = self
            .transitions
            .iter()
            .flat_map(|tr| tr.observation.iter().copied())
            .collect();
        let actions_vec: Vec<i64> = self.transitions.iter().map(|tr| tr.action).collect();
        let log_probs_vec: Vec<f32> = self.transitions.iter().map(|tr| tr.log_prob).collect();

        RolloutBatch {
            observations: Tensor::from_slice(&obs_flat)
                .reshape(&[t as i64, obs_dim])
                .to_kind(Kind::Float)
                .to_device(Device::Cpu),
            actions: Tensor::from_slice(&actions_vec).to_device(Device::Cpu),
            old_log_probs: Tensor::from_slice(&log_probs_vec)
                .to_kind(Kind::Float)
                .to_device(Device::Cpu),
            returns: Tensor::from_slice(&returns)
                .to_kind(Kind::Float)
                .to_device(Device::Cpu),
            advantages: Tensor::from_slice(&advantages)
                .to_kind(Kind::Float)
                .to_device(Device::Cpu),
        }
    }

    /// Number of transitions collected so far.
    pub fn len(&self) -> usize {
        self.transitions.len()
    }

    /// `true` if no transitions have been collected.
    pub fn is_empty(&self) -> bool {
        self.transitions.is_empty()
    }

    /// Read-only view of the collected transitions.
    pub fn transitions(&self) -> &[OnPolicyTransition] {
        &self.transitions
    }
}

/// Compute discounted returns and GAE(γ, λ) advantages.
///
/// Returns `(returns, advantages)` both of length `T`.
///
/// GAE(γ, λ=1) reduces to Monte-Carlo returns minus baseline.
/// GAE(γ, λ=0) reduces to one-step TD residuals.
pub fn compute_gae(
    transitions: &[OnPolicyTransition],
    bootstrap_value: f32,
    gamma: f64,
    lambda: f64,
) -> (Vec<f32>, Vec<f32>) {
    let t = transitions.len();
    let mut returns = vec![0.0_f32; t];
    let mut advantages = vec![0.0_f32; t];

    let mut gae = 0.0_f64;
    let mut next_value = bootstrap_value as f64;

    for i in (0..t).rev() {
        let tr = &transitions[i];
        let mask = if tr.done { 0.0_f64 } else { 1.0_f64 };
        let delta = tr.reward as f64 + gamma * next_value * mask - tr.value as f64;
        gae = delta + gamma * lambda * mask * gae;
        advantages[i] = gae as f32;
        returns[i] = (gae + tr.value as f64) as f32;
        next_value = tr.value as f64;
    }

    (returns, advantages)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ppo::{ActorCritic, PpoConfig};
    use tch::nn;

    fn make_actor_critic(obs_dim: i64, num_actions: i64) -> (nn::VarStore, ActorCritic) {
        let vs = nn::VarStore::new(Device::Cpu);
        let ac = ActorCritic::new(&vs, obs_dim, num_actions, &PpoConfig::default());
        (vs, ac)
    }

    #[test]
    fn rollout_collector_produces_correct_batch_shapes() {
        let obs_dim = 4_i64;
        let num_actions = 3_i64;
        let horizon = 8;
        let (_vs, ac) = make_actor_critic(obs_dim, num_actions);

        let mut collector = RolloutCollector::new(horizon, 0.99, 0.95);
        collector.collect(&ac, vec![0.0_f32; obs_dim as usize], |obs, _action| {
            StepOutput {
                next_observation: obs,
                reward: 1.0,
                done: false,
            }
        });

        assert_eq!(collector.len(), horizon);
        let batch = collector.into_batch(0.0);
        assert_eq!(batch.observations.size(), vec![horizon as i64, obs_dim]);
        assert_eq!(batch.actions.size(), vec![horizon as i64]);
        assert_eq!(batch.old_log_probs.size(), vec![horizon as i64]);
        assert_eq!(batch.returns.size(), vec![horizon as i64]);
        assert_eq!(batch.advantages.size(), vec![horizon as i64]);
    }

    #[test]
    fn gae_terminal_masks_correctly() {
        let transitions = vec![
            OnPolicyTransition {
                observation: vec![0.0],
                action: 0,
                log_prob: -1.0,
                value: 1.0,
                reward: 1.0,
                done: true,
            },
            OnPolicyTransition {
                observation: vec![0.0],
                action: 0,
                log_prob: -1.0,
                value: 0.5,
                reward: 0.5,
                done: false,
            },
        ];
        let (returns, _advantages) = compute_gae(&transitions, 0.0, 0.99, 0.95);
        // First transition is done → bootstrap is 0, return = reward = 1.0
        assert!((returns[0] - 1.0).abs() < 1e-5, "done mask failed: {}", returns[0]);
    }

    #[test]
    fn rollout_returns_are_finite() {
        let obs_dim = 4_i64;
        let num_actions = 3_i64;
        let horizon = 16;
        let (_vs, ac) = make_actor_critic(obs_dim, num_actions);

        let mut collector = RolloutCollector::new(horizon, 0.99, 0.95);
        collector.collect(&ac, vec![0.5_f32; obs_dim as usize], |obs, _a| StepOutput {
            next_observation: obs,
            reward: 0.1,
            done: false,
        });

        let batch = collector.into_batch(0.0);
        let returns_ok = batch.returns.isfinite().all().int64_value(&[]) != 0;
        let adv_ok = batch.advantages.isfinite().all().int64_value(&[]) != 0;
        assert!(returns_ok, "returns contain non-finite values");
        assert!(adv_ok, "advantages contain non-finite values");
    }
}
