//! Microstructure benchmark training tests (WP-022, `torch` feature).

use std::path::PathBuf;

use trolly_gym::sim::MicrostructureConfig;
use trolly_gym::train::{
    run_microstructure_train_with_checkpoints, MicrostructureCompletionRecord,
    MicrostructureTrainConfig, MicrostructureTrainSession, COMPLETED_MARKER,
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

#[test]
fn session_skips_training_when_completed_marker_present() {
    let dir = temp_dir("microstructure_completed_marker");
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

    let record = MicrostructureCompletionRecord {
        completed: true,
        tier: "zero_drift".into(),
        mean_eval_reward: 0.0,
        oracle_reward: 0.0,
        hold_baseline_reward: 0.0,
        mean_trades: 0.0,
        eval_std: 0.0,
        update_count: 10,
        eval_seeds: vec![1000],
    };
    let marker_path = dir.join(COMPLETED_MARKER);
    std::fs::write(
        &marker_path,
        serde_json::to_string_pretty(&record).expect("serialize"),
    )
    .expect("write marker");

    let mut session = MicrostructureTrainSession::resume_from(&dir, &config);
    assert!(session.is_completed());
    let (metrics, _) = session.train_step();
    assert_eq!(metrics.steps_collected, 0);
    assert_eq!(session.update_count, 0);

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn refresh_completed_manifest_lists_markers() {
    use trolly_gym::train::{refresh_completed_manifest, COMPLETED_MODELS_MANIFEST};

    let root = temp_dir("microstructure_manifest");
    let mlp_dir = root.join("microstructure_train").join("mlp");
    std::fs::create_dir_all(&mlp_dir).expect("create mlp dir");
    let record = MicrostructureCompletionRecord {
        completed: true,
        tier: "drift".into(),
        mean_eval_reward: 11.5,
        oracle_reward: 12.3,
        hold_baseline_reward: 0.0,
        mean_trades: 1.0,
        eval_std: 0.0,
        update_count: 20,
        eval_seeds: vec![2000],
    };
    std::fs::write(
        mlp_dir.join(COMPLETED_MARKER),
        serde_json::to_string_pretty(&record).expect("serialize"),
    )
    .expect("write marker");

    refresh_completed_manifest(&root);
    let manifest_path = root.join(COMPLETED_MODELS_MANIFEST);
    assert!(manifest_path.exists());
    let text = std::fs::read_to_string(&manifest_path).expect("read manifest");
    assert!(text.contains("microstructure"));
    assert!(text.contains("mlp"));

    let _ = std::fs::remove_dir_all(&root);
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
