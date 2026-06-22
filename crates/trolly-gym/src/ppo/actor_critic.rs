//! Shared-trunk actor–critic MLP (categorical policy + scalar value).

use tch::nn;
use tch::{Kind, Tensor};

use super::config::PpoConfig;

/// Feed-forward actor–critic with a shared hidden trunk and separate heads.
pub struct ActorCritic {
    trunk: nn::Sequential,
    policy_head: nn::Linear,
    value_head: nn::Linear,
}

impl ActorCritic {
    /// Build a new actor–critic on `path` using `config` layer sizes.
    pub fn new(path: nn::Path, config: &PpoConfig) -> Self {
        let mut trunk = nn::seq();
        let mut in_dim = config.obs_dim;
        for (idx, &hidden) in config.hidden_dims.iter().enumerate() {
            trunk = trunk
                .add(nn::linear(
                    &path / format!("trunk_{idx}"),
                    in_dim,
                    hidden,
                    Default::default(),
                ))
                .add_fn(|xs| xs.tanh());
            in_dim = hidden;
        }

        let policy_head = nn::linear(
            &path / "policy",
            in_dim,
            config.num_actions,
            Default::default(),
        );
        let value_head = nn::linear(&path / "value", in_dim, 1, Default::default());

        Self {
            trunk,
            policy_head,
            value_head,
        }
    }

    /// Forward pass returning `(values, logits)` with shapes `[batch]` and `[batch, num_actions]`.
    pub fn forward(&self, observations: &Tensor) -> (Tensor, Tensor) {
        let features = observations.apply(&self.trunk);
        let values = features.apply(&self.value_head).squeeze_dim(-1);
        let logits = features.apply(&self.policy_head);
        (values, logits)
    }

    /// Sample actions and return `(actions, log_probs, values)`.
    pub fn act(&self, observations: &Tensor) -> (Tensor, Tensor, Tensor) {
        let (values, logits) = self.forward(observations);
        let probs = logits.softmax(-1, Kind::Float);
        let actions = probs.multinomial(1, true).squeeze_dim(-1);
        let log_probs = logits.log_softmax(-1, Kind::Float);
        let index = actions.unsqueeze(-1);
        let action_log_probs = log_probs.gather(-1, &index, false).squeeze_dim(-1);
        (actions, action_log_probs, values)
    }
}
