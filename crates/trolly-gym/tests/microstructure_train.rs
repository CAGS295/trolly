//! Microstructure benchmark training tests (WP-022, `torch` feature).

use std::path::PathBuf;

use trolly_gym::sim::MicrostructureConfig;
use trolly_gym::train::{
    run_microstructure_train_with_checkpoints, MicrostructureTrainConfig,
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
