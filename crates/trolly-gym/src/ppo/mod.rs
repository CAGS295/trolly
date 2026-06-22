//! Proximal Policy Optimization (PPO) and WoLF-PPO actor–critic training.
//!
//! Gated behind the crate `torch` feature (`tch` / libtorch).

mod actor_critic;
mod batch;
mod config;
mod loss;
mod trainer;

pub use actor_critic::ActorCritic;
pub use batch::RolloutBatch;
pub use config::{OptimizerKind, PpoConfig, WolfPpoConfig};
pub use loss::{losses_are_finite, ppo_loss, PpoLossBreakdown};
pub use trainer::{PpoTrainer, WolfPpoTrainer};

#[cfg(test)]
mod tests {
    use super::*;
    use tch::{Device, Kind, Tensor};

    #[test]
    fn actor_critic_forward_shapes() {
        let config = PpoConfig {
            obs_dim: 6,
            num_actions: 3,
            hidden_dims: vec![20, 20],
            ..Default::default()
        };
        let vs = tch::nn::VarStore::new(Device::Cpu);
        let net = ActorCritic::new(vs.root(), &config);

        let batch = 5;
        let obs = Tensor::randn([batch, config.obs_dim], (Kind::Float, Device::Cpu));
        let (values, logits) = net.forward(&obs);
        assert_eq!(values.size(), [batch]);
        assert_eq!(logits.size(), [batch, config.num_actions]);
    }

    #[test]
    fn act_returns_aligned_tensors() {
        let config = PpoConfig::default();
        let vs = tch::nn::VarStore::new(Device::Cpu);
        let net = ActorCritic::new(vs.root(), &config);
        let obs = Tensor::randn([4, config.obs_dim], (Kind::Float, Device::Cpu));
        let (actions, log_probs, values) = net.act(&obs);
        assert_eq!(actions.size(), [4]);
        assert_eq!(log_probs.size(), [4]);
        assert_eq!(values.size(), [4]);
    }
}
