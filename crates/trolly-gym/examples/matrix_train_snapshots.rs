//! Time-bounded matrix-game training with checkpoint snapshots.
//!
//! Trains both MLP and Liquid (LNN) backends on weighted Matching Pennies and RPS.
//! Each run resumes from `latest.safetensors` when present so timed loops continue
//! the same model instead of restarting from scratch.
//!
//! ```bash
//! export LIBTORCH=/path/to/libtorch
//! export LD_LIBRARY_PATH=$LIBTORCH/lib:$LD_LIBRARY_PATH
//! cargo run -p trolly-gym --features torch --example matrix_train_snapshots
//! ```
//!
//! Optional env vars:
//! - `TRAIN_DURATION_SECS` (default 90) — wall-clock budget per game × architecture
//! - `CHECKPOINT_DIR` (default `./checkpoints/matrix_train`) — output root

use std::path::PathBuf;
use std::time::{Duration, Instant};

use trolly_gym::games::{
    matching_pennies::{matching_pennies_weighted, WEIGHTED_NES},
    rock_paper_scissors::{rps_weighted, WEIGHTED_NES as RPS_WEIGHTED_NES},
    SelfPlayConfig, WolfPpoSelfPlaySession,
};
use trolly_gym::ppo::{ActorCriticArchitecture, PpoConfig, WolfPpoConfig};
use trolly_gym::train::checkpoint::LATEST_CHECKPOINT;

fn main() {
    let duration = Duration::from_secs(
        std::env::var("TRAIN_DURATION_SECS")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(90),
    );
    let root: PathBuf = std::env::var("CHECKPOINT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("checkpoints/matrix_train"));

    std::fs::create_dir_all(&root).expect("create checkpoint root");

    let mp = matching_pennies_weighted();
    let rps = rps_weighted();
    let games: [(&str, &trolly_gym::games::MatrixGame, &[f64]); 2] = [
        ("matching_pennies_weighted", &mp, &WEIGHTED_NES),
        ("rps_weighted", &rps, &RPS_WEIGHTED_NES),
    ];
    let architectures = [
        (ActorCriticArchitecture::Mlp, "mlp"),
        (ActorCriticArchitecture::Liquid, "liquid"),
    ];

    for (architecture, arch_name) in architectures {
        for (name, game, nes) in &games {
            train_game_timed(arch_name, architecture, name, game, nes, duration, &root);
        }
    }

    println!("All runs complete. Checkpoints under {}", root.display());
}

fn train_game_timed(
    arch_name: &str,
    architecture: ActorCriticArchitecture,
    name: &str,
    game: &trolly_gym::games::MatrixGame,
    nes: &[f64],
    duration: Duration,
    root: &PathBuf,
) {
    let out_dir = root.join(arch_name).join(name);
    std::fs::create_dir_all(&out_dir).expect("create game checkpoint dir");

    let config = SelfPlayConfig {
        num_updates: 10,
        batch_size: 64,
        ppo_config: PpoConfig {
            architecture,
            ppo_epochs: 2,
            ..Default::default()
        },
    };
    let wolf = WolfPpoConfig {
        ppo: config.ppo_config.clone(),
        ..WolfPpoConfig::default().with_alpha_lose(0.1)
    };

    let resumed = out_dir.join(LATEST_CHECKPOINT).exists();
    let mut session = WolfPpoSelfPlaySession::resume_from(&out_dir, game, &config, wolf);

    let start = Instant::now();
    let mut batch = 0_u64;

    println!(
        "=== {arch_name}/{name}: training for {}s ({}) ===",
        duration.as_secs(),
        if resumed { "resumed" } else { "fresh" },
    );

    while start.elapsed() < duration {
        let distances = session.run_updates(game, nes, 10, &out_dir, false);
        batch += 1;
        let elapsed = start.elapsed().as_secs_f64();
        let last_dist = distances.last().copied().unwrap_or(0.0);
        println!(
            "  batch {batch}: {} updates (total {}), nes_dist={last_dist:.4}, elapsed={elapsed:.1}s",
            distances.len(),
            session.update_count,
        );
    }

    session.finalize_checkpoints(&out_dir);
    println!("  final snapshot: {}/final_row_player.safetensors", out_dir.display());
}
