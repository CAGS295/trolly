//! Integration smoke tests for PPO / WoLF-PPO (requires `--features torch`).

use trolly_gym::{
    compute_advantages, ActorCritic, PpoConfig, PpoTrainer, RolloutBatch, WolfPpoConfig,
    WolfPpoTrainer, cpu_device,
};
use tch::{nn, Kind, Tensor};

#[test]
fn ppo_trainer_policy_update_finite() {
    let device = cpu_device();
    let config = PpoConfig {
        ppo_epochs: 2,
        hidden_layers: vec![20, 20],
        ..Default::default()
    };
    let mut trainer = PpoTrainer::new(2, 2, config, device);
    let rewards = vec![1.0, 0.0, -1.0, 0.5];
    let values = vec![0.0; 4];
    let (advantages, returns) = compute_advantages(&rewards, &values, 0.99);
    let batch = RolloutBatch::from_slices(
        &[0.0_f32, 0.0, 1.0, 0.0, 0.0, 1.0, 0.5, 0.5],
        2,
        &[0, 1, 0, 1],
        &[-0.69, -0.69, -0.69, -0.69],
        &advantages,
        &returns,
        &values,
        device,
    );
    let metrics = trainer.policy_update(&batch);
    assert!(metrics.total_loss.is_finite());
    assert!(metrics.entropy.is_finite());
}

#[test]
fn actor_critic_hidden_layers_twenty_twenty() {
    let device = cpu_device();
    let vs = nn::VarStore::new(device);
    let ac = ActorCritic::new(&vs.root(), 1, 2, &[20, 20]);
    let obs = Tensor::from_slice(&[0.5_f32]).reshape(&[1, 1]);
    let probs = ac.policy_probs(&obs);
    assert_eq!(probs.size(), vec![1, 2]);
    let sum = probs.sum(Kind::Float).double_value(&[]);
    assert!((sum - 1.0).abs() < 1e-5);
}

#[test]
fn wolf_ppo_uses_distinct_learning_rates() {
    let device = cpu_device();
    let config = WolfPpoConfig {
        alpha_lose: 0.08,
        ppo: PpoConfig {
            ppo_epochs: 1,
            ..Default::default()
        },
        ..Default::default()
    };
    let mut trainer = WolfPpoTrainer::new(1, 2, config, device);
    trainer.set_estimated_nes_payoff(0.0);

    let batch_win = RolloutBatch::from_slices(
        &[1.0],
        1,
        &[0],
        &[-0.5],
        &[0.1],
        &[0.2],
        &[0.1],
        device,
    );
    let mut win = batch_win.clone();
    win.mean_return = 1.0;
    let m_win = trainer.policy_update(&win);
    assert!(m_win.wolf_winning);
    assert!((m_win.active_learning_rate - 0.02).abs() < 1e-9);

    let batch_lose = RolloutBatch::from_slices(
        &[0.0],
        1,
        &[1],
        &[-0.5],
        &[-0.1],
        &[-0.2],
        &[-0.1],
        device,
    );
    let mut lose = batch_lose.clone();
    lose.mean_return = -1.0;
    let m_lose = trainer.policy_update(&lose);
    assert!(!m_lose.wolf_winning);
    assert!((m_lose.active_learning_rate - 0.08).abs() < 1e-9);
}
