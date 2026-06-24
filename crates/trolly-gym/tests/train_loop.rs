//! End-to-end training loop and checkpoint tests (WP-020, `torch` feature).

use trolly_gym::{
    collect_env_rollout, load_checkpoint, save_checkpoint, smoke_train_loop, ActorCritic,
    CheckpointMeta, Env, EnvConfig, PpoConfig, WolfPpoConfig, WolfPpoTrainingDriver,
};
use trolly_strategy::{DepthUpdate, PriceLevel, RecordingEgress, StreamEvent};

#[test]
fn checkpoint_roundtrip_restores_forward_pass() {
    use tch::{nn, Device, Kind, Tensor};

    let device = Device::Cpu;
    let obs_dim = 6_i64;
    let action_count = 3_i64;
    let config = PpoConfig::default();

    let vs1 = nn::VarStore::new(device);
    let model1 = ActorCritic::new(
        &vs1.root(),
        obs_dim,
        action_count,
        &trolly_gym::default_hidden_layers(),
        &config,
    );

    let obs = Tensor::randn(&[3, obs_dim], (Kind::Float, device));
    let (logits1, values1) = model1.forward(&obs);

    let dir = std::env::temp_dir().join(format!("trolly_gym_it_ckpt_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let base = dir.join("policy");

    let meta = CheckpointMeta::with_default_hidden(obs_dim, action_count);
    save_checkpoint(&base, &vs1, &meta).unwrap();

    let mut vs2 = nn::VarStore::new(device);
    let (_loaded, model2) = load_checkpoint(&base, &mut vs2, &config).unwrap();
    let (logits2, values2) = model2.forward(&obs);

    assert!((&logits1 - &logits2).abs().max().double_value(&[]) < 1e-6);
    assert!((&values1 - &values2).abs().max().double_value(&[]) < 1e-6);

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn short_end_to_end_env_train_loop() {
    use tch::{nn, Device};

    let device = Device::Cpu;
    let mut config = EnvConfig::new("BTCUSDT");
    config.window_frames = 1;
    let mut env = Env::new(config, RecordingEgress::default());

    let events = [
        StreamEvent::Depth(DepthUpdate {
            symbol: "BTCUSDT".into(),
            bids: vec![PriceLevel {
                price: "100".into(),
                qty: "1".into(),
            }],
            asks: vec![PriceLevel {
                price: "102".into(),
                qty: "1".into(),
            }],
            update_id: Some(1),
        }),
        StreamEvent::Depth(DepthUpdate {
            symbol: "BTCUSDT".into(),
            bids: vec![PriceLevel {
                price: "101".into(),
                qty: "1".into(),
            }],
            asks: vec![PriceLevel {
                price: "103".into(),
                qty: "1".into(),
            }],
            update_id: Some(2),
        }),
    ];

    let vs = nn::VarStore::new(device);
    let obs_dim = 7_i64;
    let ppo = PpoConfig {
        ppo_epochs: 1,
        use_adam: false,
        ..Default::default()
    };
    let model = ActorCritic::new(
        &vs.root(),
        obs_dim,
        3,
        &trolly_gym::default_hidden_layers(),
        &ppo,
    );
    let mut opt = ActorCritic::build_optimizer(&vs, &ppo);
    let mut driver = WolfPpoTrainingDriver::from_wolf_config(WolfPpoConfig {
        ppo,
        alpha_lose: 0.01,
        ..Default::default()
    });

    for _ in 0..2 {
        let rollout = collect_env_rollout(&mut env, &model, &events);
        assert!(!rollout.is_empty());
        let metrics = driver.update_from_rollout(&model, &mut opt, &rollout, 0.0, None, device);
        assert!(metrics.policy_loss.is_finite());
        assert!(metrics.entropy.is_finite());
        assert!(metrics.learning_rate.is_some());
    }
}

#[test]
fn smoke_train_loop_driver() {
    let log = smoke_train_loop(tch::Device::Cpu);
    assert_eq!(log.len(), 3);
    assert!(log.last().unwrap().total_loss.is_finite());
}
