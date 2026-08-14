//! Time-bounded microstructure training with checkpoint snapshots.
//!
//! Trains both MLP and Liquid (LNN) backends on the synthetic order-book env.
//! Each run resumes from `latest.safetensors` when present and stops when
//! completion criteria are satisfied (`completed.json`).
//!
//! ```bash
//! export LIBTORCH=/path/to/libtorch
//! export LD_LIBRARY_PATH=$LIBTORCH/lib:$LD_LIBRARY_PATH
//! cargo run -p trolly-gym --features torch --example microstructure_train_snapshots
//! ```
//!
//! Optional env vars:
//! - `TRAIN_DURATION_SECS` (default 90)
//! - `CHECKPOINT_DIR` (default `./checkpoints/microstructure_train`)
//! - `MICROSTRUCTURE_MID_DRIFT` (default `0.1` — drift tier; set `0` for zero-drift)

use std::path::PathBuf;
use std::time::{Duration, Instant};

use trolly_gym::ppo::{ActorCriticArchitecture, PpoConfig, WolfPpoConfig};
use trolly_gym::sim::MicrostructureConfig;
use trolly_gym::train::{
    checkpoint::LATEST_CHECKPOINT, refresh_completed_manifest, MicrostructureCompletionCriteria,
    MicrostructureTrainConfig, MicrostructureTrainSession, TrainDriverConfig, COMPLETED_MARKER,
    COMPLETED_MODELS_MANIFEST,
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

    let mid_drift = std::env::var("MICROSTRUCTURE_MID_DRIFT")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(0.1_f32);

    let sim = MicrostructureConfig {
        mid_drift,
        mid_noise: if mid_drift.abs() > f32::EPSILON {
            0.0
        } else {
            0.25
        },
        ..Default::default()
    };
    let driver = TrainDriverConfig {
        obs_dim: sim.obs_dim(),
        num_actions: 3,
        horizon: sim.episode_steps,
        ..Default::default()
    };

    for (architecture, arch_name) in [
        (ActorCriticArchitecture::Mlp, "mlp"),
        (ActorCriticArchitecture::Liquid, "liquid"),
    ] {
        train_arch_timed(arch_name, architecture, &sim, &driver, duration, &root);
    }

    let manifest_root = root.parent().unwrap_or(&root);
    refresh_completed_manifest(manifest_root);
    println!(
        "Completed manifest: {}/{}",
        manifest_root.display(),
        COMPLETED_MODELS_MANIFEST
    );

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

    let completion = MicrostructureCompletionCriteria::for_config(sim);
    let tier = MicrostructureCompletionCriteria::tier_name(sim);

    let train_config = MicrostructureTrainConfig {
        sim: sim.clone(),
        driver: driver.clone(),
        wolf,
        num_updates: 1,
        checkpoint_dir: Some(out_root.clone()),
        completion: Some(completion),
        device: trolly_gym::device::resolve_training_device().unwrap_or(tch::Device::Cpu),
        ..Default::default()
    };

    let resumed = out_root.join(LATEST_CHECKPOINT).exists();
    let mut session = MicrostructureTrainSession::resume_from(&out_root, &train_config);

    if session.is_completed() {
        if let Some(record) = session.completion_record() {
            println!(
                "=== microstructure/{arch_name}: already complete (tier={tier}, reward={:.4}, oracle={:.4}) ===",
                record.mean_eval_reward, record.oracle_reward
            );
        }
        return;
    }

    let start = Instant::now();

    println!(
        "=== microstructure/{arch_name}: training for {}s (tier={tier}, obs_dim={}, {}) ===",
        duration.as_secs(),
        driver.obs_dim,
        if resumed { "resumed" } else { "fresh" },
    );

    while start.elapsed() < duration && !session.is_completed() {
        let (_metrics, stats) = session.train_step();
        session.save_checkpoint(&out_root, false);
        println!(
            "  update {}: reward={:.4} steps={} pos={}",
            session.update_count,
            stats.total_reward,
            stats.steps,
            stats.final_position,
        );

        if let Some((eval, completed)) = session.maybe_evaluate_completion(&out_root) {
            println!(
                "  eval: mean_reward={:.4} oracle={:.4} hold={:.4} trades={:.2}",
                eval.mean_reward,
                eval.oracle_reward,
                eval.hold_baseline,
                eval.mean_trades,
            );
            if completed {
                println!("  training complete -> {}/{}", out_root.display(), COMPLETED_MARKER);
                break;
            }
        }
    }

    if !session.is_completed() {
        session.finalize_checkpoints(&out_root);
        println!("  final snapshot: {}/final.safetensors", out_root.display());
    }
}
