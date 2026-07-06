//! Time-bounded microstructure training with checkpoint snapshots.
//!
//! ```bash
//! export LIBTORCH=/path/to/libtorch
//! export LD_LIBRARY_PATH=$LIBTORCH/lib:$LD_LIBRARY_PATH
//! cargo run -p trolly-gym --features torch --example microstructure_train_snapshots
//! ```

use std::path::PathBuf;
use std::time::{Duration, Instant};

use trolly_gym::sim::MicrostructureConfig;
use trolly_gym::train::{
    run_microstructure_train_with_checkpoints, MicrostructureTrainConfig, TrainDriverConfig,
};
use trolly_gym::ppo::WolfPpoConfig;

fn main() {
    let duration = Duration::from_secs(
        std::env::var("TRAIN_DURATION_SECS")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(60),
    );
    let root: PathBuf = std::env::var("CHECKPOINT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("checkpoints/microstructure_train"));

    std::fs::create_dir_all(&root).expect("create checkpoint root");

    let sim = MicrostructureConfig::default();
    let driver = TrainDriverConfig {
        obs_dim: sim.obs_dim(),
        num_actions: 3,
        horizon: 64,
        ..Default::default()
    };

    let start = Instant::now();
    let mut update = 0_u64;

    println!(
        "=== microstructure: training for {}s (obs_dim={}) ===",
        duration.as_secs(),
        driver.obs_dim
    );

    while start.elapsed() < duration {
        let batch_dir = root.join(format!("update_{update:04}"));
        let (_metrics, stats, paths) = run_microstructure_train_with_checkpoints(
            MicrostructureTrainConfig {
                sim: sim.clone(),
                driver: driver.clone(),
                wolf: WolfPpoConfig::default(),
                num_updates: 1,
                checkpoint_dir: Some(batch_dir.clone()),
            },
        );
        update += 1;
        if let Some(last) = stats.last() {
            println!(
                "  update {update}: reward={:.4} steps={} pos={} checkpoint={}",
                last.total_reward,
                last.steps,
                last.final_position,
                paths[0].display(),
            );
        }
    }

    println!("Done. Checkpoints under {}", root.display());
}
