//! Shared-trunk actor–critic MLP with categorical policy head.

use tch::{nn, Tensor};

/// Fully connected actor–critic network: shared hidden trunk, policy logits, scalar value.
pub struct ActorCritic {
    trunk: nn::Sequential,
    policy_head: nn::Linear,
    value_head: nn::Linear,
    n_actions: i64,
}

impl ActorCritic {
    /// Build an actor–critic MLP with the given observation dimension and action count.
    pub fn new(
        vs: nn::Path,
        obs_dim: i64,
        n_actions: i64,
        hidden_layers: &[i64],
    ) -> Self {
        let mut trunk = nn::seq();
        let mut in_dim = obs_dim;
        for (layer_idx, &width) in hidden_layers.iter().enumerate() {
            trunk = trunk
                .add(nn::linear(
                    &vs / format!("fc_{layer_idx}"),
                    in_dim,
                    width,
                    Default::default(),
                ))
                .add_fn(|x| x.relu());
            in_dim = width;
        }

        let policy_head = nn::linear(&vs / "policy", in_dim, n_actions, Default::default());
        let value_head = nn::linear(&vs / "value", in_dim, 1, Default::default());

        Self {
            trunk,
            policy_head,
            value_head,
            n_actions,
        }
    }

    /// Number of discrete actions.
    pub fn n_actions(&self) -> i64 {
        self.n_actions
    }

    /// Forward pass: `(logits [batch, n_actions], values [batch, 1])`.
    pub fn forward(&self, obs: &Tensor) -> (Tensor, Tensor) {
        let features = obs.apply(&self.trunk);
        let logits = features.apply(&self.policy_head);
        let values = features.apply(&self.value_head);
        (logits, values)
    }

    /// Log-probabilities and entropy for taken actions under the current policy.
    pub fn evaluate_actions(&self, obs: &Tensor, actions: &Tensor) -> (Tensor, Tensor, Tensor) {
        let (logits, values) = self.forward(obs);
        let log_probs_all = logits.log_softmax(-1, tch::Kind::Float);
        let log_probs = log_probs_all.gather(1, &actions.unsqueeze(1), false).squeeze_dim(1);

        let probs = logits.softmax(-1, tch::Kind::Float);
        let entropy = -(probs * log_probs_all)
            .sum_dim_intlist(&[-1i64][..], true, tch::Kind::Float)
            .squeeze_dim(1);

        (log_probs, entropy, values.squeeze_dim(1))
    }
}
