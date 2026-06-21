//! Actor–critic MLP with categorical policy head and scalar value head.

use tch::{nn, nn::Module, nn::OptimizerConfig, Device, Kind, Tensor};

use super::config::PpoConfig;

/// Two-hidden-layer actor–critic network (default `[20, 20]` per paper matrix games).
pub struct ActorCritic {
    shared: nn::Sequential,
    policy_head: nn::Linear,
    value_head: nn::Linear,
    action_count: i64,
}

impl ActorCritic {
    pub fn new(
        vs: &nn::Path,
        obs_dim: i64,
        action_count: i64,
        hidden: &[i64],
        config: &PpoConfig,
    ) -> Self {
        let mut shared = nn::seq();
        let mut in_dim = obs_dim;
        for &h in hidden {
            shared = shared.add(nn::linear(vs / "shared", in_dim, h, Default::default()));
            shared = shared.add_fn(move |xs| xs.relu());
            in_dim = h;
        }

        let trunk_dim = hidden.last().copied().unwrap_or(obs_dim);
        let policy_head = nn::linear(vs / "policy", trunk_dim, action_count, Default::default());
        let value_head = nn::linear(vs / "value", trunk_dim, 1, Default::default());

        let _ = config; // reserved for future init schemes

        Self {
            shared,
            policy_head,
            value_head,
            action_count,
        }
    }

    pub fn action_count(&self) -> i64 {
        self.action_count
    }

    /// Forward pass returning `(logits, value)` with shape `[batch, actions]` and `[batch, 1]`.
    pub fn forward(&self, obs: &Tensor) -> (Tensor, Tensor) {
        let trunk = obs.apply(&self.shared);
        let logits = trunk.apply(&self.policy_head);
        let value = trunk.apply(&self.value_head);
        (logits, value)
    }

    /// Sample actions and return `(actions, log_probs, values, entropy)`.
    pub fn act(&self, obs: &Tensor) -> (Tensor, Tensor, Tensor, Tensor) {
        let (logits, values) = self.forward(obs);
        let dist = logits.log_softmax(-1, Kind::Float);
        let probs = dist.exp();
        let actions = probs.multinomial(1, true).squeeze_dim(-1);
        let log_probs = dist.gather(1, &actions.unsqueeze(-1), false).squeeze_dim(-1);
        let entropy = -(probs * dist).sum_dim_intlist(&[-1i64][..], false, Kind::Float);
        (actions, log_probs, values.squeeze_dim(-1), entropy)
    }

    /// Evaluate stored actions: `(log_probs, values, entropy)`.
    pub fn evaluate_actions(&self, obs: &Tensor, actions: &Tensor) -> (Tensor, Tensor, Tensor) {
        let (logits, values) = self.forward(obs);
        let dist = logits.log_softmax(-1, Kind::Float);
        let probs = dist.exp();
        let log_probs = dist.gather(1, &actions.unsqueeze(-1), false).squeeze_dim(-1);
        let entropy = -(probs * dist).sum_dim_intlist(&[-1i64][..], false, Kind::Float);
        (log_probs, values.squeeze_dim(-1), entropy)
    }

    /// Expected payoff under the current policy for a payoff matrix `payoffs[action, opponent]`.
    pub fn expected_payoff(&self, obs: &Tensor, payoffs: &Tensor, opponent_probs: &Tensor) -> f64 {
        let (logits, _) = self.forward(obs);
        let policy_probs = logits.softmax(-1, Kind::Float);
        // payoffs: [actions, opp_actions], opponent_probs: [opp_actions]
        let action_values = payoffs.matmul(opponent_probs.unsqueeze(-1)).squeeze_dim(-1);
        policy_probs
            .mul(&action_values)
            .sum(Kind::Float)
            .double_value(&[])
    }

    pub fn build_optimizer(vs: &nn::VarStore, config: &PpoConfig) -> nn::Optimizer {
        if config.use_adam {
            nn::Adam::default().build(vs, config.learning_rate).unwrap()
        } else {
            nn::Sgd::default().build(vs, config.learning_rate).unwrap()
        }
    }
}

pub fn default_hidden_layers() -> Vec<i64> {
    vec![20, 20]
}

pub fn cpu_device() -> Device {
    Device::Cpu
}
