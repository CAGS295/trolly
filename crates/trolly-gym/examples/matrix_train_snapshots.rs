//! Time-bounded matrix-game training with checkpoint snapshots.
//!
//! ```bash
//! export LIBTORCH=/path/to/libtorch
//! export LD_LIBRARY_PATH=$LIBTORCH/lib:$LD_LIBRARY_PATH
//! cargo run -p trolly-gym --features torch --example matrix_train_snapshots
//! ```
//!
//! Optional env vars:
//! - `TRAIN_DURATION_SECS` (default 180) — wall-clock budget per game
//! - `CHECKPOINT_DIR` (default `./checkpoints/matrix_train`) — output root

use std::path::PathBuf;
use std::time::{Duration, Instant};

use trolly_gym::games::{
    matching_pennies::{matching_pennies_weighted, WEIGHTED_NES},
    rock_paper_scissors::{rps_weighted, WEIGHTED_NES as RPS_WEIGHTED_NES},
    run_wolf_ppo_self_play_with_checkpoints, SelfPlayConfig,
};
use trolly_gym::ppo::{PpoConfig, WolfPpoConfig};

fn main() {
    let duration = Duration::from_secs(
        std::env::var("TRAIN_DURATION_SECS")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(180),
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

    for (name, game, nes) in games {
        train_game_timed(name, &game, nes, duration, &root);
    }

    println!("All runs complete. Checkpoints under {}", root.display());
}

fn train_game_timed(
    name: &str,
    game: &trolly_gym::games::MatrixGame,
    nes: &[f64],
    duration: Duration,
    root: &PathBuf,
) {
    let out_dir = root.join(name);
    std::fs::create_dir_all(&out_dir).expect("create game checkpoint dir");

    let config = SelfPlayConfig {
        num_updates: 10,
        batch_size: 64,
        ppo_config: PpoConfig {
            ppo_epochs: 2,
            ..Default::default()
        },
    };
    let wolf = WolfPpoConfig::default().with_alpha_lose(0.1);

    let start = Instant::now();
    let mut batch = 0_u64;
    let mut all_paths = Vec::new();

    println!("=== {name}: training for {}s ===", duration.as_secs());

    while start.elapsed() < duration {
        let batch_dir = out_dir.join(format!("batch_{batch:04}"));
        let (result, paths) = run_wolf_ppo_self_play_with_checkpoints(
            game,
            nes,
            config.clone(),
            wolf.clone(),
            &batch_dir,
        );
        batch += 1;
        let elapsed = start.elapsed().as_secs_f64();
        println!(
            "  batch {batch}: {} updates, max_nes_dist_last10={:.4}, elapsed={elapsed:.1}s",
            paths.len(),
            result.max_distance_last_10,
        );
        all_paths.extend(paths);
    }

    let final_path = out_dir.join("final_row_player.safetensors");
    if let Some(last) = all_paths.last() {
        std::fs::copy(last, &final_path).expect("copy final snapshot");
        println!("  final snapshot: {}", final_path.display());
    }
}
