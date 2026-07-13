//! Microstructure benchmark training tests (WP-022, `torch` feature).

use std::path::PathBuf;

use trolly_gym::sim::MicrostructureConfig;
use trolly_gym::train::{
    run_microstructure_train_with_checkpoints, MicrostructureTrainConfig, MicrostructureTrainSession,
};

#[test]
fn microstructure_train_saves_checkpoints() {
    let dir = temp_dir("microstructure_checkpoints");
    let sim = MicrostructureConfig {
        episode_steps: 32,
        window_frames: 1,
        ..Default::default()
    };
    let (metrics, stats, paths) = run_microstructure_train_with_checkpoints(
        MicrostructureTrainConfig {
            sim: sim.clone(),
            driver: trolly_gym::train::TrainDriverConfig {
                obs_dim: sim.obs_dim(),
                num_actions: 3,
                horizon: 32,
                ..Default::default()
            },
            num_updates: 3,
            checkpoint_dir: Some(dir.clone()),
            ..Default::default()
        },
    );

    assert_eq!(metrics.len(), 3);
    assert_eq!(paths.len(), 3);
    assert_eq!(stats.len(), 3);
    for path in &paths {
        assert!(path.exists(), "missing checkpoint: {}", path.display());
    }
    assert!(metrics.last().unwrap().policy_loss.is_finite());
    assert!(stats.last().unwrap().total_reward.is_finite());

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn microstructure_session_resumes_continuous_training() {
    use tch::{Device, Kind, Tensor};

    let dir = temp_dir("microstructure_session_resume");
    let sim = MicrostructureConfig {
        episode_steps: 32,
        window_frames: 1,
        ..Default::default()
    };
    let config = MicrostructureTrainConfig {
        sim: sim.clone(),
        driver: trolly_gym::train::TrainDriverConfig {
            obs_dim: sim.obs_dim(),
            num_actions: 3,
            horizon: 32,
            ..Default::default()
        },
        num_updates: 1,
        checkpoint_dir: Some(dir.clone()),
        ..Default::default()
    };

    let mut first = MicrostructureTrainSession::new(&config);
    for _ in 0..3 {
        first.train_step();
        first.save_checkpoint(&dir, false);
    }
    let logits_after_first = forward_logits(&first, sim.obs_dim());

    let resumed = MicrostructureTrainSession::resume_from(&dir, &config);
    assert_eq!(forward_logits(&resumed, sim.obs_dim()), logits_after_first);

    let mut continued = resumed;
    for _ in 0..3 {
        continued.train_step();
        continued.save_checkpoint(&dir, false);
    }
    assert_ne!(forward_logits(&continued, sim.obs_dim()), logits_after_first);
    assert!(dir.join("latest.safetensors").exists());

    let _ = std::fs::remove_dir_all(&dir);

    fn forward_logits(session: &MicrostructureTrainSession, obs_dim: i64) -> Vec<f64> {
        let obs = Tensor::zeros(&[1, obs_dim], (Kind::Float, Device::Cpu));
        let (logits, _) = session.actor_critic().forward(&obs);
        logits
            .to_kind(Kind::Double)
            .view([-1])
            .iter::<f64>()
            .unwrap()
            .collect()
    }
}

fn temp_dir(label: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("trolly_gym_{label}_{nanos}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create temp dir");
    dir
}
