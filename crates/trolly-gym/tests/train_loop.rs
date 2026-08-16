//! End-to-end training loop and checkpoint tests (WP-020/WP-021/WP-022, `torch` feature).

use std::path::PathBuf;

use trolly_gym::games::{
    matching_pennies::{matching_pennies_weighted, WEIGHTED_NES},
    run_wolf_ppo_self_play_with_checkpoints, SelfPlayConfig, WolfPpoSelfPlaySession,
};
use trolly_gym::ppo::{ActorCritic, ActorCriticArchitecture, PpoConfig, WolfPpoConfig};
use trolly_gym::train::{
    load_checkpoint, save_checkpoint, smoke_train_loop, SmokeTrainConfig, StepOutput,
    TrainDriverConfig, WolfPpoTrainDriver,
};
use trolly_gym::{run_offline_policy_harness, Action, CheckpointOrHoldPolicy, Env, EnvConfig};
use trolly_strategy::{envelope_message, DepthUpdate, PriceLevel, RecordingEgress, StreamEvent};

fn depth_event(symbol: &str, bid: &str, ask: &str, update_id: u64) -> StreamEvent {
    StreamEvent::Depth(DepthUpdate {
        symbol: symbol.into(),
        bids: vec![PriceLevel {
            price: bid.into(),
            qty: "1".into(),
        }],
        asks: vec![PriceLevel {
            price: ask.into(),
            qty: "1".into(),
        }],
        update_id: Some(update_id),
    })
}

fn action_from_index(index: i64) -> Action {
    match index.rem_euclid(3) {
        1 => Action::Buy,
        2 => Action::Sell,
        _ => Action::Hold,
    }
}

#[test]
fn checkpoint_roundtrip_restores_forward_pass() {
    use tch::{nn, Device, Kind, Tensor};

    let device = Device::Cpu;
    let obs_dim = 6_i64;
    let action_count = 3_i64;
    let config = PpoConfig::default();

    let vs1 = nn::VarStore::new(device);
    let model1 = ActorCritic::new(&vs1, obs_dim, action_count, &config);

    let obs = Tensor::randn(&[3, obs_dim], (Kind::Float, device));
    let (logits1, values1) = model1.forward(&obs);

    let dir = temp_dir("checkpoint_roundtrip");
    let path = dir.join("policy.safetensors");
    save_checkpoint(&vs1, &path).unwrap();

    let mut vs2 = nn::VarStore::new(device);
    let model2 = ActorCritic::new(&vs2, obs_dim, action_count, &config);
    load_checkpoint(&mut vs2, &path).unwrap();
    let (logits2, values2) = model2.forward(&obs);

    assert!((&logits1 - &logits2).abs().max().double_value(&[]) < 1e-6);
    assert!((&values1 - &values2).abs().max().double_value(&[]) < 1e-6);

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn checkpoint_policy_harness_loads_latest_and_steps_injected_stream() {
    use tch::{nn, Device};

    let obs_dim = 7_i64;
    let config = PpoConfig::default();
    let vs = nn::VarStore::new(Device::Cpu);
    let _model = ActorCritic::new(&vs, obs_dim, Action::COUNT, &config);
    let dir = temp_dir("checkpoint_policy_harness");
    save_checkpoint(&vs, dir.join(trolly_gym::train::LATEST_CHECKPOINT)).unwrap();

    let policy = CheckpointOrHoldPolicy::from_latest_checkpoint_dir(&dir, obs_dim, config).unwrap();
    let mut env_config = EnvConfig::new("BTCUSDT");
    env_config.window_frames = 1;
    let mut env = Env::new(env_config, RecordingEgress::default());

    let steps = run_offline_policy_harness(
        &mut env,
        &policy,
        [envelope_message(&depth_event("BTCUSDT", "100", "101", 1))],
    )
    .unwrap();

    assert_eq!(steps.len(), 1);
    assert_eq!(env.egress().dispatched.len(), 1);
    assert!(matches!(
        &env.egress().dispatched[0],
        trolly_strategy::OutboundMessage::Subscribe { .. }
            | trolly_strategy::OutboundMessage::OrderRequest { .. }
    ));

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn short_end_to_end_env_train_loop() {
    let mut config = EnvConfig::new("BTCUSDT");
    config.window_frames = 1;
    let mut env = Env::new(config, RecordingEgress::default());

    let events = [
        depth_event("BTCUSDT", "100", "102", 1),
        depth_event("BTCUSDT", "101", "103", 2),
    ];

    for event in &events {
        assert!(env.ingest_event(event));
    }
    let initial_obs = env.observation_window().flattened();
    assert!(!initial_obs.is_empty());

    let mut driver = WolfPpoTrainDriver::new(
        TrainDriverConfig {
            obs_dim: initial_obs.len() as i64,
            num_actions: 3,
            horizon: 4,
            gamma: 0.99,
            gae_lambda: 0.95,
        },
        WolfPpoConfig {
            ppo: PpoConfig {
                ppo_epochs: 1,
                use_adam: false,
                ..Default::default()
            },
            alpha_lose: 0.01,
            alpha_win: 0.01 / 4.0,
            ..Default::default()
        },
    );

    let metrics = driver.train_step(
        initial_obs,
        |obs, action_idx| {
            let step = env.step(action_from_index(action_idx)).unwrap();
            StepOutput {
                next_observation: if step.observation.is_empty() {
                    obs
                } else {
                    step.observation
                },
                reward: step.reward,
                done: step.done,
            }
        },
        0.0,
        None,
    );

    assert!(metrics.policy_loss.is_finite());
    assert!(metrics.entropy.is_finite());
    assert!(metrics.active_lr.is_finite());
    assert_eq!(metrics.steps_collected, 4);
}

#[test]
fn liquid_train_loop_driver_smoke() {
    let obs_dim = 4_i64;
    let mut driver = WolfPpoTrainDriver::new(
        TrainDriverConfig {
            obs_dim,
            num_actions: 3,
            horizon: 4,
            gamma: 0.99,
            gae_lambda: 0.95,
        },
        WolfPpoConfig {
            ppo: PpoConfig {
                architecture: ActorCriticArchitecture::Liquid,
                ppo_epochs: 1,
                ..Default::default()
            },
            alpha_lose: 0.01,
            alpha_win: 0.01 / 4.0,
            ..Default::default()
        },
    );

    let metrics = driver.train_step(
        vec![0.0_f32; obs_dim as usize],
        |obs, action_idx| StepOutput {
            next_observation: obs,
            reward: if action_idx == 0 { 0.25 } else { -0.1 },
            done: false,
        },
        0.0,
        Some(&[1.0 / 3.0, 1.0 / 3.0, 1.0 / 3.0]),
    );

    assert!(metrics.policy_loss.is_finite());
    assert!(metrics.value_loss.is_finite());
    assert!(metrics.entropy.is_finite());
    assert!(metrics.nes_distance.unwrap().is_finite());
}

#[test]
fn short_matrix_game_train_saves_checkpoints() {
    let game = matching_pennies_weighted();
    let nes = &WEIGHTED_NES;
    let config = SelfPlayConfig {
        num_updates: 3,
        batch_size: 8,
        ppo_config: PpoConfig {
            ppo_epochs: 1,
            ..Default::default()
        },
    };
    let dir = temp_dir("matrix_game_checkpoints");

    let (result, paths) =
        run_wolf_ppo_self_play_with_checkpoints(&game, nes, config, WolfPpoConfig::default(), &dir);

    assert_eq!(paths.len(), 3);
    for path in &paths {
        assert!(path.exists(), "missing checkpoint: {}", path.display());
    }
    assert!(result.max_distance_last_10.is_finite());
    assert_eq!(result.distances_per_update.len(), 3);

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn matrix_session_resumes_continuous_training() {
    use tch::{Device, Kind, Tensor};

    let game = matching_pennies_weighted();
    let nes = &WEIGHTED_NES;
    let config = SelfPlayConfig {
        num_updates: 5,
        batch_size: 8,
        ppo_config: PpoConfig {
            ppo_epochs: 1,
            ..Default::default()
        },
    };
    let wolf = WolfPpoConfig::default();
    let dir = temp_dir("matrix_session_resume");

    let mut first = WolfPpoSelfPlaySession::new(&game, &config, wolf.clone());
    first.run_updates(&game, nes, 5, &dir, false);
    let probs_after_first = policy_probs_from_session(&first);

    let resumed = WolfPpoSelfPlaySession::resume_from(&dir, &game, &config, wolf.clone());
    let probs_resumed = policy_probs_from_session(&resumed);
    assert_eq!(probs_resumed, probs_after_first);

    let mut continued = resumed;
    continued.run_updates(&game, nes, 5, &dir, false);
    let probs_after_more = policy_probs_from_session(&continued);
    assert_ne!(probs_after_more, probs_resumed);
    assert!(dir.join("latest.safetensors").exists());
    assert!(dir.join("latest_opponent.safetensors").exists());

    let _ = std::fs::remove_dir_all(&dir);

    fn policy_probs_from_session(session: &WolfPpoSelfPlaySession) -> Vec<f64> {
        let obs = Tensor::zeros(&[1, 1], (Kind::Float, Device::Cpu));
        let (logits, _) = session.p1.inner.actor_critic.forward(&obs);
        let probs = logits.softmax(-1, Kind::Float).squeeze();
        let n = probs.size()[0] as usize;
        (0..n).map(|i| probs.double_value(&[i as i64])).collect()
    }
}

#[test]
fn smoke_train_loop_driver() {
    let dir = temp_dir("smoke_train_loop");
    let (log, paths) = smoke_train_loop(SmokeTrainConfig {
        num_steps: 3,
        checkpoint_dir: Some(dir.clone()),
        ..Default::default()
    });

    assert_eq!(log.len(), 3);
    assert_eq!(paths.len(), 3);
    assert!(log.last().unwrap().policy_loss.is_finite());
    for path in &paths {
        assert!(path.exists(), "missing checkpoint: {}", path.display());
    }

    let _ = std::fs::remove_dir_all(&dir);
}

fn temp_dir(label: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!(
        "trolly_gym_{label}_{}_{nanos}",
        label = label,
        nanos = nanos,
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create temp dir");
    dir
}
