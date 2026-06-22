//! Matrix-game validation harness tests (Ratcliffe et al. IEEE CoG 2019, WP-019).

use trolly_gym::games::{euclidean_distance, max_distance_over_last, MatrixGame, MatrixGameKind};

// ---------------------------------------------------------------------------
// Offline smoke tests (no libtorch; always run under default `cargo test`)
// ---------------------------------------------------------------------------

#[test]
fn nes_probabilities_match_paper_table_iib() {
    let mp = MatrixGame::new(MatrixGameKind::WeightedMatchingPennies);
    assert!((mp.nes_probabilities()[0] - 0.4).abs() < 1e-9);

    let rps = MatrixGame::new(MatrixGameKind::WeightedRockPaperScissors);
    let nes = rps.nes_probabilities();
    assert!((nes[0] - 0.2).abs() < 1e-9);
    assert!((nes[1] - 0.4).abs() < 1e-9);
    assert!((nes[2] - 0.4).abs() < 1e-9);
}

#[test]
fn standard_matching_pennies_nes_is_uniform() {
    let game = MatrixGame::new(MatrixGameKind::MatchingPennies);
    let nes = game.nes_probabilities();
    assert!((nes[0] - 0.5).abs() < 1e-9);
    assert!((nes[1] - 0.5).abs() < 1e-9);
}

#[test]
fn euclidean_distance_metric_at_equilibrium_is_zero() {
    let game = MatrixGame::new(MatrixGameKind::WeightedMatchingPennies);
    let nes = game.nes_probabilities();
    assert!((euclidean_distance(nes, nes) - 0.0).abs() < 1e-12);
}

#[test]
fn max_distance_over_last_ten_matches_table_i_methodology() {
    let window = (0..15).map(|i| i as f64 * 0.01).collect::<Vec<_>>();
    let max_last = max_distance_over_last(&window, 10);
    let expected = window[5..].iter().copied().fold(f64::NEG_INFINITY, f64::max);
    assert!((max_last - expected).abs() < 1e-9);
}

// ---------------------------------------------------------------------------
// Torch-gated self-play smoke (requires `--features torch` + libtorch)
// ---------------------------------------------------------------------------

#[cfg(feature = "torch")]
mod torch {
    use trolly_gym::games::{
        run_ppo_self_play, run_wolf_ppo_self_play, HarnessConfig, MatrixGameKind,
    };

    #[test]
    fn wolf_ppo_smoke_training_step_and_finite_nes_distance() {
        let config = HarnessConfig::smoke(MatrixGameKind::WeightedMatchingPennies, 42);
        let result = run_wolf_ppo_self_play(&config);
        assert!(result.final_distance().is_finite());
        assert!(result.max_distance_last_n.is_finite());
        assert_eq!(result.distances.len(), config.num_policy_updates);
    }

    #[test]
    fn ppo_smoke_training_step_and_finite_nes_distance() {
        let config = HarnessConfig::smoke(MatrixGameKind::RockPaperScissors, 99);
        let result = run_ppo_self_play(&config);
        assert!(result.final_distance().is_finite());
        assert!(result.max_distance_last_n.is_finite());
    }

    /// Extended benchmark: WoLF-PPO should stay closer to the NES than PPO on
    /// weighted Matching Pennies (paper Table I trend).
    ///
    /// Run locally with full paper settings:
    /// ```bash
    /// export LIBTORCH=/path/to/libtorch   # or LIBTORCH_USE_PYTORCH=1
    /// cargo test -p trolly-gym --features torch matrix_games \
    ///   -- --ignored --nocapture
    /// ```
    ///
    /// For 50-run reproduction, set `TROLLY_MATRIX_BENCHMARK_RUNS=50`.
    #[test]
    #[ignore = "extended matrix-game benchmark; run with --ignored locally"]
    fn weighted_matching_pennies_wolf_closer_to_nes_than_ppo() {
        let num_runs = benchmark_run_count();
        for &alpha_lose in &[0.1_f64, 0.01] {
            let summary = trolly_gym::games::benchmark_ppo_vs_wolf(
                MatrixGameKind::WeightedMatchingPennies,
                alpha_lose,
                num_runs,
            );
            eprintln!(
                "weighted MP α_LOSE={alpha_lose}: PPO mean max dist {:.4}, WoLF-PPO {:.4} ({num_runs} runs)",
                summary.ppo_mean_max_distance, summary.wolf_mean_max_distance
            );
            assert!(
                summary.wolf_mean_max_distance < summary.ppo_mean_max_distance,
                "WoLF-PPO should be closer to NES than PPO at α_LOSE={alpha_lose}"
            );
        }
    }

    fn benchmark_run_count() -> usize {
        std::env::var("TROLLY_MATRIX_BENCHMARK_RUNS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(10)
    }
}
