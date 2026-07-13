//! Time-bounded microstructure training with checkpoint snapshots.
//!
//! Trains both MLP and Liquid (LNN) backends on the synthetic order-book env.
//! Each run resumes from `latest.safetensors` when present.
//!
//! ```bash
//! export LIBTORCH=/path/to/libtorch
//! export LD_LIBRARY_PATH=$LIBTORCH/lib:$LD_LIBRARY_PATH
//! cargo run -p trolly-gym --features torch --example microstructure_train_snapshots
//! ```

use std::path::PathBuf;
use std::time::{Duration, Instant};

use trolly_gym::ppo::{ActorCriticArchitecture, PpoConfig, WolfPpoConfig};
use trolly_gym::sim::MicrostructureConfig;
use trolly_gym::train::{
    checkpoint::LATEST_CHECKPOINT, MicrostructureTrainConfig, MicrostructureTrainSession,
    TrainDriverConfig,
};

fn main() {
    let duration = Duration::from_secs(
        std::env::var("TRAIN_DURATION_SECS")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(90),
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

    for (architecture, arch_name) in [
        (ActorCriticArchitecture::Mlp, "mlp"),
        (ActorCriticArchitecture::Liquid, "liquid"),
    ] {
        train_arch_timed(arch_name, architecture, &sim, &driver, duration, &root);
    }

    println!("Done. Checkpoints under {}", root.display());
}

fn train_arch_timed(
    arch_name: &str,
    architecture: ActorCriticArchitecture,
    sim: &MicrostructureConfig,
    driver: &TrainDriverConfig,
    duration: Duration,
    root: &PathBuf,
) {
    let out_root = root.join(arch_name);
    std::fs::create_dir_all(&out_root).expect("create arch checkpoint dir");

    let wolf = WolfPpoConfig {
        ppo: PpoConfig {
            architecture,
            ..Default::default()
        },
        ..Default::default()
    };

    let train_config = MicrostructureTrainConfig {
        sim: sim.clone(),
        driver: driver.clone(),
        wolf,
        num_updates: 1,
        checkpoint_dir: Some(out_root.clone()),
    };

    let resumed = out_root.join(LATEST_CHECKPOINT).exists();
    let mut session = MicrostructureTrainSession::resume_from(&out_root, &train_config);

    let start = Instant::now();

    println!(
        "=== microstructure/{arch_name}: training for {}s (obs_dim={}, {}) ===",
        duration.as_secs(),
        driver.obs_dim,
        if resumed { "resumed" } else { "fresh" },
    );

    while start.elapsed() < duration {
        let (_metrics, stats) = session.train_step();
        session.save_checkpoint(&out_root, false);
        println!(
            "  update {}: reward={:.4} steps={} pos={}",
            session.update_count,
            stats.total_reward,
            stats.steps,
            stats.final_position,
        );
    }

    session.finalize_checkpoints(&out_root);
    println!("  final snapshot: {}/final.safetensors", out_root.display());
}
