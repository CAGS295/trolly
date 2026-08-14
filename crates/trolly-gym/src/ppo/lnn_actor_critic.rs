//! Liquid neural network actor-critic for discrete action spaces.

use tch::{nn, nn::Module, Kind, Tensor};

use super::config::PpoConfig;

/// Actor-critic with a fixed-step liquid state update.
///
/// The liquid cell starts from a zero hidden state for each observation batch,
/// repeatedly mixes an input-driven candidate state into the hidden state, then
/// feeds the final state through optional readout layers and the policy/value
/// heads. This keeps the same stateless training contract as the MLP model.
pub struct LiquidActorCritic {
    input_drive: nn::Linear,
    recurrent_drive: nn::Linear,
    input_gate: nn::Linear,
    tau_log: Tensor,
    readout: Vec<nn::Linear>,
    policy_head: nn::Linear,
    value_head: nn::Linear,
    hidden_size: i64,
    liquid_steps: usize,
}

impl LiquidActorCritic {
    /// Build the liquid actor-critic and register parameters in `vs`.
    ///
    /// The first `config.hidden_sizes` entry is the liquid state width; any
    /// remaining entries become readout MLP layers before the heads.
    pub fn new(vs: &nn::VarStore, obs_dim: i64, num_actions: i64, config: &PpoConfig) -> Self {
        let p = vs.root();
        let hidden_size = config
            .hidden_sizes
            .first()
            .copied()
            .unwrap_or_else(|| obs_dim.max(num_actions).max(1));
        let liquid_steps = config.liquid_steps.max(1);

        let input_drive = nn::linear(
            &p / "liquid_input_drive",
            obs_dim,
            hidden_size,
            Default::default(),
        );
        let recurrent_drive = nn::linear(
            &p / "liquid_recurrent_drive",
            hidden_size,
            hidden_size,
            Default::default(),
        );
        let input_gate = nn::linear(
            &p / "liquid_input_gate",
            obs_dim,
            hidden_size,
            Default::default(),
        );
        let tau_log = p.var("liquid_tau_log", &[hidden_size], nn::Init::Const(0.0));

        let mut readout = Vec::new();
        let mut in_dim = hidden_size;
        for (i, &h) in config.hidden_sizes.iter().skip(1).enumerate() {
            readout.push(nn::linear(
                &p / format!("liquid_readout_{i}"),
                in_dim,
                h,
                Default::default(),
            ));
            in_dim = h;
        }

        let policy_head = nn::linear(
            &p / "liquid_policy",
            in_dim,
            num_actions,
            Default::default(),
        );
        let value_head = nn::linear(&p / "liquid_value", in_dim, 1, Default::default());

        Self {
            input_drive,
            recurrent_drive,
            input_gate,
            tau_log,
            readout,
            policy_head,
            value_head,
            hidden_size,
            liquid_steps,
        }
    }

    pub fn device(&self) -> tch::Device {
        self.policy_head.ws.device()
    }

    /// Forward pass.
    ///
    /// Returns `(logits [batch, num_actions], values [batch])`.
    pub fn forward(&self, obs: &Tensor) -> (Tensor, Tensor) {
        let x = obs.to_kind(Kind::Float);
        let batch = x.size()[0];
        let mut state = Tensor::zeros(&[batch, self.hidden_size], (Kind::Float, x.device()));
        let input_gate = self.input_gate.forward(&x).sigmoid();
        let tau = self.tau_log.exp() + 1.0;
        let alpha = (&input_gate / &tau).clamp(0.0, 1.0);

        for _ in 0..self.liquid_steps {
            let candidate =
                (self.input_drive.forward(&x) + self.recurrent_drive.forward(&state)).tanh();
            state = &state + &alpha * (candidate - &state);
        }

        let mut features = state;
        for layer in &self.readout {
            features = layer.forward(&features).tanh();
        }

        let logits = self.policy_head.forward(&features);
        let value = self.value_head.forward(&features).squeeze_dim(-1);
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
        let entropy = -(&probs * &log_probs).sum_dim_intlist(&[-1i64][..], false, Kind::Float);
        (action_log_probs, entropy)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ppo::config::ActorCriticArchitecture;
    use tch::Device;

    fn liquid_config() -> PpoConfig {
        PpoConfig {
            architecture: ActorCriticArchitecture::Liquid,
            ppo_epochs: 1,
            ..Default::default()
        }
    }

    #[test]
    fn forward_shapes_batch() {
        let vs = nn::VarStore::new(Device::Cpu);
        let ac = LiquidActorCritic::new(&vs, 4, 3, &liquid_config());
        let obs = Tensor::zeros(&[8, 4], (Kind::Float, Device::Cpu));
        let (logits, values) = ac.forward(&obs);
        assert_eq!(logits.size(), vec![8, 3], "logits shape mismatch");
        assert_eq!(values.size(), vec![8], "values shape mismatch");
    }

    #[test]
    fn evaluate_actions_shapes_and_finite() {
        let vs = nn::VarStore::new(Device::Cpu);
        let ac = LiquidActorCritic::new(&vs, 4, 3, &liquid_config());
        let obs = Tensor::randn(&[16, 4], (Kind::Float, Device::Cpu));
        let actions = Tensor::zeros(&[16], (Kind::Int64, Device::Cpu));
        let (log_probs, entropy) = ac.evaluate_actions(&obs, &actions);
        assert_eq!(log_probs.size(), vec![16]);
        assert_eq!(entropy.size(), vec![16]);
        assert!(log_probs.isfinite().all().int64_value(&[]) != 0);
        assert!(entropy.isfinite().all().int64_value(&[]) != 0);
    }
}
