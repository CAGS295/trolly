//! Feed-forward actor–critic MLP (categorical policy + scalar value head).

use tch::{nn, Device, Kind, Tensor};

/// Shared-body actor–critic with a stochastic categorical policy head.
pub struct ActorCritic {
    shared: nn::Sequential,
    policy_head: nn::Linear,
    value_head: nn::Linear,
    num_actions: i64,
}

impl ActorCritic {
    pub fn new(vs: &nn::Path, obs_dim: i64, num_actions: i64, hidden_layers: &[i64]) -> Self {
        let mut shared = nn::seq();
        let mut in_dim = obs_dim;
        for &width in hidden_layers {
            shared = shared
                .add(nn::linear(vs / "shared", in_dim, width, Default::default()))
                .add_fn(|x| x.tanh());
            in_dim = width;
        }
        let policy_head = nn::linear(vs / "policy", in_dim, num_actions, Default::default());
        let value_head = nn::linear(vs / "value", in_dim, 1, Default::default());
        Self {
            shared,
            policy_head,
            value_head,
            num_actions,
        }
    }

    pub fn num_actions(&self) -> i64 {
        self.num_actions
    }

    /// Forward pass returning `(logits, value)` with shape `[batch, num_actions]` and `[batch, 1]`.
    pub fn forward(&self, obs: &Tensor) -> (Tensor, Tensor) {
        let body = obs.apply(&self.shared);
        let logits = body.apply(&self.policy_head);
        let value = body.apply(&self.value_head);
        (logits, value)
    }

    /// Sample actions and return `(actions, log_probs, entropy, values)`.
    pub fn act(&self, obs: &Tensor) -> (Tensor, Tensor, Tensor, Tensor) {
        let (logits, values) = self.forward(obs);
        let log_probs_all = logits.log_softmax(-1, Kind::Float);
        let probs = log_probs_all.exp();
        let actions = probs.multinomial(1, true).squeeze_dim(-1);
        let selected_log_probs = log_probs_all.gather(1, &actions.unsqueeze(1), false).squeeze_dim(1);
        let entropy = -(probs * log_probs_all)
            .sum_dim_intlist(&[-1i64][..], true, Kind::Float)
            .squeeze_dim(1);
        (actions, selected_log_probs, entropy, values.squeeze_dim(1))
    }

    /// Evaluate stored actions: `(log_probs, entropy, values)`.
    pub fn evaluate_actions(&self, obs: &Tensor, actions: &Tensor) -> (Tensor, Tensor, Tensor) {
        let (logits, values) = self.forward(obs);
        let log_probs_all = logits.log_softmax(-1, Kind::Float);
        let probs = log_probs_all.exp();
        let selected_log_probs = log_probs_all.gather(1, &actions.unsqueeze(1), false).squeeze_dim(1);
        let entropy = -(probs * log_probs_all)
            .sum_dim_intlist(&[-1i64][..], true, Kind::Float)
            .squeeze_dim(1);
        (selected_log_probs, entropy, values.squeeze_dim(1))
    }

    /// Deterministic argmax policy probabilities (for NES distance metrics in WP-019).
    pub fn policy_probs(&self, obs: &Tensor) -> Tensor {
        let (logits, _) = self.forward(obs);
        logits.softmax(-1, Kind::Float)
    }
}

/// Build an actor–critic on CPU with the given dimensions.
pub fn new_actor_critic(
    vs: &nn::VarStore,
    obs_dim: i64,
    num_actions: i64,
    hidden_layers: &[i64],
) -> ActorCritic {
    ActorCritic::new(&vs.root(), obs_dim, num_actions, hidden_layers)
}

pub fn cpu_device() -> Device {
    Device::Cpu
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ppo::config::PpoConfig;

    #[test]
    fn forward_shapes_and_act() {
        let device = cpu_device();
        let vs = nn::VarStore::new(device);
        let hidden = PpoConfig::default().hidden_layers;
        let ac = ActorCritic::new(&vs.root(), 3, 2, &hidden);
        let obs = Tensor::zeros(&[5, 3], (Kind::Float, device));
        let (logits, values) = ac.forward(&obs);
        assert_eq!(logits.size(), vec![5, 2]);
        assert_eq!(values.size(), vec![5, 1]);

        let (actions, log_probs, entropy, vals) = ac.act(&obs);
        assert_eq!(actions.size(), vec![5]);
        assert_eq!(log_probs.size(), vec![5]);
        assert_eq!(entropy.size(), vec![5]);
        assert_eq!(vals.size(), vec![5]);
        assert!(entropy.mean(Kind::Float).double_value(&[]).is_finite());
    }

    #[test]
    fn evaluate_actions_matches_batch() {
        let device = cpu_device();
        let vs = nn::VarStore::new(device);
        let ac = ActorCritic::new(&vs.root(), 2, 3, &[20, 20]);
        let obs = Tensor::zeros(&[4, 2], (Kind::Float, device));
        let actions = Tensor::from_slice(&[0_i64, 1, 2, 0]).to_kind(Kind::Int64);
        let (log_probs, entropy, values) = ac.evaluate_actions(&obs, &actions);
        assert_eq!(log_probs.size(), vec![4]);
        assert_eq!(entropy.size(), vec![4]);
        assert_eq!(values.size(), vec![4]);
    }
}
