//! PPO and WoLF-PPO policy optimization (`torch` feature).

mod actor_critic;
mod config;
mod loss;
mod trainer;

pub use actor_critic::{default_hidden_layers, ActorCritic, cpu_device};
pub use config::{PpoConfig, WolfPpoConfig};
pub use loss::PpoLossBreakdown;
pub use trainer::{compute_advantages, tensor_from_vec_f64, tensor_from_vec_i64, PpoTrainer, RolloutBatch, WolfPpoTrainer};

#[cfg(test)]
mod tests {
    use super::*;
    use tch::{nn, Device, Kind, Tensor};

    fn synthetic_batch(obs_dim: i64, batch: i64, _actions: i64) -> RolloutBatch {
        let observations = Tensor::randn(&[batch, obs_dim], (Kind::Float, Device::Cpu));
        let action_tensor = Tensor::zeros(&[batch], (Kind::Int64, Device::Cpu));
        let old_log_probs = Tensor::full(&[batch], -0.5, (Kind::Float, Device::Cpu));
        let advantages = Tensor::randn(&[batch], (Kind::Float, Device::Cpu));
        let returns = Tensor::randn(&[batch], (Kind::Float, Device::Cpu));
        RolloutBatch::new(
            observations,
            action_tensor,
            old_log_probs,
            advantages,
            returns,
        )
    }

    #[test]
    fn actor_critic_forward_shapes() {
        let vs = nn::VarStore::new(Device::Cpu);
        let config = PpoConfig::default();
        let model = ActorCritic::new(&vs.root(), 4, 2, &default_hidden_layers(), &config);
        let obs = Tensor::randn(&[8, 4], (Kind::Float, Device::Cpu));
        let (logits, value) = model.forward(&obs);
        assert_eq!(logits.size(), vec![8, 2]);
        assert_eq!(value.size(), vec![8, 1]);
    }

    #[test]
    fn ppo_loss_finite_on_synthetic_batch() {
        let vs = nn::VarStore::new(Device::Cpu);
        let config = PpoConfig {
            ppo_epochs: 2,
            ..Default::default()
        };
        let model = ActorCritic::new(&vs.root(), 4, 2, &default_hidden_layers(), &config);
        let mut opt = ActorCritic::build_optimizer(&vs, &config);
        let batch = synthetic_batch(4, 16, 2);

        let trainer = PpoTrainer::new(config);
        let breakdown = trainer.policy_update(&model, &mut opt, &batch);

        assert!(breakdown.policy_loss.is_finite());
        assert!(breakdown.value_loss.is_finite());
        assert!(breakdown.entropy.is_finite());
        assert!(breakdown.total.is_finite());
    }

    #[test]
    fn wolf_lr_switches_on_payoff_vs_estimate() {
        let mut trainer = WolfPpoTrainer::new(WolfPpoConfig {
            alpha_lose: 0.04,
            ..Default::default()
        });
        trainer.nes_payoff_estimate = 0.5;

        assert!((trainer.select_learning_rate(0.6) - 0.01).abs() < 1e-9); // α_WIN
        assert!((trainer.select_learning_rate(0.4) - 0.04).abs() < 1e-9); // α_LOSE
    }

    #[test]
    fn wolf_ppo_update_applies_dynamic_lr() {
        let vs = nn::VarStore::new(Device::Cpu);
        let wolf_config = WolfPpoConfig {
            ppo: PpoConfig {
                ppo_epochs: 1,
                use_adam: false,
                ..Default::default()
            },
            alpha_lose: 0.08,
            ..Default::default()
        };
        let model = ActorCritic::new(
            &vs.root(),
            4,
            2,
            &default_hidden_layers(),
            &wolf_config.ppo,
        );
        let mut opt = ActorCritic::build_optimizer(&vs, &wolf_config.ppo);
        let batch = synthetic_batch(4, 8, 2);

        let mut trainer = WolfPpoTrainer::new(wolf_config);
        trainer.nes_payoff_estimate = 1.0;
        let breakdown = trainer.policy_update(&model, &mut opt, &batch, 2.0);

        assert!((trainer.last_learning_rate - 0.02).abs() < 1e-9); // α_WIN = 0.08/4
        assert!(breakdown.total.is_finite());
    }
}
