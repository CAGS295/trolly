//! Actor-critic MLP for discrete action spaces.

use tch::{nn, nn::Module, Kind, Tensor};

use super::config::PpoConfig;

/// Actor-critic MLP: shared trunk → categorical policy logits + scalar value.
///
/// Architecture (tanh activations on shared layers):
/// ```text
/// obs → [hidden_sizes] → policy_head → logits [num_actions]
///                      ↘ value_head  → value  [1]
/// ```
pub struct ActorCritic {
    shared: Vec<nn::Linear>,
    policy_head: nn::Linear,
    value_head: nn::Linear,
}

impl ActorCritic {
    /// Build the network and register parameters in `vs`.
    ///
    /// - `obs_dim`: flattened observation size.
    /// - `num_actions`: discrete action count (policy logits dimension).
    pub fn new(vs: &nn::VarStore, obs_dim: i64, num_actions: i64, config: &PpoConfig) -> Self {
        let p = vs.root();
        let mut shared = Vec::new();
        let mut in_dim = obs_dim;
        for (i, &h) in config.hidden_sizes.iter().enumerate() {
            shared.push(nn::linear(
                &p / format!("shared_{i}"),
                in_dim,
                h,
                Default::default(),
            ));
            in_dim = h;
        }
        let policy_head = nn::linear(&p / "policy", in_dim, num_actions, Default::default());
        let value_head = nn::linear(&p / "value", in_dim, 1, Default::default());
        Self {
            shared,
            policy_head,
            value_head,
        }
    }

    /// Forward pass.
    ///
    /// Returns `(logits [batch, num_actions], values [batch])`.
    pub fn forward(&self, obs: &Tensor) -> (Tensor, Tensor) {
        let mut x = obs.to_kind(Kind::Float);
        for layer in &self.shared {
            x = layer.forward(&x).tanh();
        }
        let logits = self.policy_head.forward(&x);
        let value = self.value_head.forward(&x).squeeze_dim(-1);
        (logits, value)
    }

    /// Sample one action per observation and return its log-probability.
    ///
    /// Returns `(actions [batch], log_probs [batch])`.
    pub fn action_and_log_prob(&self, obs: &Tensor) -> (Tensor, Tensor) {
        let (logits, _) = self.forward(obs);
        let action = logits
            .softmax(-1, Kind::Float)
            .multinomial(1, true)
            .squeeze_dim(-1);
        let log_prob = logits
            .log_softmax(-1, Kind::Float)
            .gather(-1, &action.unsqueeze(-1), false)
            .squeeze_dim(-1);
        (action, log_prob)
    }

    /// Evaluate log-probabilities and per-sample entropy for a batch of actions.
    ///
    /// Returns `(action_log_probs [batch], entropy [batch])`.
    pub fn evaluate_actions(&self, obs: &Tensor, actions: &Tensor) -> (Tensor, Tensor) {
        let (logits, _) = self.forward(obs);
        let log_probs = logits.log_softmax(-1, Kind::Float);
        let action_log_probs = log_probs
            .gather(-1, &actions.unsqueeze(-1), false)
            .squeeze_dim(-1);
        let probs = logits.softmax(-1, Kind::Float);
        // H(π) = −Σ_a π(a|s) log π(a|s)
        let entropy = -(&probs * &log_probs).sum_dim_intlist(&[-1i64][..], false, Kind::Float);
        (action_log_probs, entropy)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tch::Device;

    fn default_config() -> PpoConfig {
        PpoConfig::default()
    }

    #[test]
    fn forward_shapes_batch() {
        let vs = nn::VarStore::new(Device::Cpu);
        let ac = ActorCritic::new(&vs, 4, 3, &default_config());
        let obs = Tensor::zeros(&[8, 4], (Kind::Float, Device::Cpu));
        let (logits, values) = ac.forward(&obs);
        assert_eq!(logits.size(), vec![8, 3], "logits shape mismatch");
        assert_eq!(values.size(), vec![8], "values shape mismatch");
    }

    #[test]
    fn forward_shapes_single() {
        let vs = nn::VarStore::new(Device::Cpu);
        let ac = ActorCritic::new(&vs, 7, 3, &default_config());
        let obs = Tensor::zeros(&[1, 7], (Kind::Float, Device::Cpu));
        let (logits, values) = ac.forward(&obs);
        assert_eq!(logits.size(), vec![1, 3]);
        assert_eq!(values.size(), vec![1]);
    }

    #[test]
    fn action_log_prob_shapes() {
        let vs = nn::VarStore::new(Device::Cpu);
        let ac = ActorCritic::new(&vs, 4, 3, &default_config());
        let obs = Tensor::zeros(&[5, 4], (Kind::Float, Device::Cpu));
        let (action, log_prob) = ac.action_and_log_prob(&obs);
        assert_eq!(action.size(), vec![5]);
        assert_eq!(log_prob.size(), vec![5]);
    }

    #[test]
    fn evaluate_actions_shapes_and_finite() {
        let vs = nn::VarStore::new(Device::Cpu);
        let ac = ActorCritic::new(&vs, 4, 3, &default_config());
        let obs = Tensor::randn(&[16, 4], (Kind::Float, Device::Cpu));
        let actions = Tensor::zeros(&[16], (Kind::Int64, Device::Cpu));
        let (log_probs, entropy) = ac.evaluate_actions(&obs, &actions);
        assert_eq!(log_probs.size(), vec![16]);
        assert_eq!(entropy.size(), vec![16]);
        // All log-probs and entropies should be finite (0-d bool tensor → i64)
        assert!(
            log_probs.isfinite().all().int64_value(&[]) != 0,
            "non-finite log_prob detected"
        );
        assert!(
            entropy.isfinite().all().int64_value(&[]) != 0,
            "non-finite entropy detected"
        );
    }
}
