//! Matrix-game validation harness tests (WP-019).

use trolly_gym::{
    euclidean_distance, max_distance_last_n, mean_std, MatrixGame, MatrixGameKind,
};

#[test]
fn all_paper_games_have_valid_payoffs_and_nes() {
    for kind in MatrixGameKind::all() {
        let game = MatrixGame::from_kind(kind);
        assert_eq!(game.payoffs.len(), game.action_count());
        assert_eq!(game.nes.len(), game.action_count());
        for row in &game.payoffs {
            assert_eq!(row.len(), game.action_count());
        }
        let sum: f64 = game.nes.iter().sum();
        assert!((sum - 1.0).abs() < 1e-9, "{kind:?} NES must sum to 1");
    }
}

#[test]
fn nes_distance_zero_for_exact_match() {
    let game = MatrixGame::from_kind(MatrixGameKind::MatchingPenniesWeighted);
    assert!((euclidean_distance(&game.nes, &game.nes) - 0.0).abs() < 1e-12);
}

#[test]
fn max_distance_metric_matches_table_i_tail_window() {
    let trace = vec![0.05, 0.12, 0.08, 0.15, 0.09];
    assert!((max_distance_last_n(&trace, 10) - 0.15).abs() < 1e-12);
    assert!((max_distance_last_n(&trace, 3) - 0.15).abs() < 1e-12);
}

#[test]
fn mean_std_computes_sample_statistics() {
    let (mean, std) = mean_std(&[0.1, 0.3, 0.5]);
    assert!((mean - 0.3).abs() < 1e-12);
    assert!(std > 0.0);
}

#[cfg(feature = "torch")]
mod torch {
    use trolly_gym::{
        mean_std, run_matrix_experiment, run_matrix_experiments, MatrixExperimentConfig,
        MatrixGameKind, MatrixTrainerKind, DEFAULT_LAST_N_POLICY_UPDATES,
    };

    #[test]
    fn wolf_ppo_smoke_training_step_and_finite_nes_distance() {
        let config = MatrixExperimentConfig::smoke(
            MatrixGameKind::MatchingPenniesWeighted,
            MatrixTrainerKind::WolfPpo,
            42,
        );
        let result = run_matrix_experiment(&config);
        assert_eq!(result.nes_distances.len(), config.num_policy_updates);
        assert!(result.max_nes_distance_last_n.is_finite());
        assert!(result.max_nes_distance_last_n >= 0.0);
    }

    #[test]
    fn ppo_smoke_training_completes_on_weighted_rps() {
        let config = MatrixExperimentConfig::smoke(
            MatrixGameKind::RockPaperScissorsWeighted,
            MatrixTrainerKind::Ppo,
            99,
        );
        let result = run_matrix_experiment(&config);
        assert!(!result.nes_distances.is_empty());
        assert!(result.max_nes_distance_last_n.is_finite());
    }

    #[test]
    fn multi_run_experiment_supports_fifty_seeds() {
        let config = MatrixExperimentConfig::smoke(
            MatrixGameKind::MatchingPenniesStandard,
            MatrixTrainerKind::WolfPpo,
            0,
        );
        let results = run_matrix_experiments(config, 50, 1_000);
        assert_eq!(results.len(), 50);
        assert!(results.iter().all(|r| r.max_nes_distance_last_n.is_finite()));
    }

    /// Extended benchmark reproducing paper Table I trend on weighted Matching Pennies
    /// (WoLF-PPO closer to NES than PPO at α_LOSE ∈ {0.1, 0.01}).
    ///
    /// Run locally:
    /// ```bash
    /// export LIBTORCH=/tmp/libtorch
    /// export LD_LIBRARY_PATH=/tmp/libtorch/lib
    /// export RUSTUP_TOOLCHAIN=stable
    /// unset LIBTORCH_USE_PYTORCH
    /// cargo test -p trolly-gym --features torch matrix_games::torch::weighted_matching_pennies_wolf_beats_ppo -- --ignored --nocapture
    /// ```
    #[test]
    #[ignore]
    fn weighted_matching_pennies_wolf_beats_ppo() {
        const RUNS: usize = 50;
        const BASE_SEED: u64 = 2024;

        for alpha_lose in [0.1_f64, 0.01] {
            let ppo_config = MatrixExperimentConfig::benchmark(
                MatrixGameKind::MatchingPenniesWeighted,
                MatrixTrainerKind::Ppo,
                alpha_lose,
                BASE_SEED,
            );
            let wolf_config = MatrixExperimentConfig::benchmark(
                MatrixGameKind::MatchingPenniesWeighted,
                MatrixTrainerKind::WolfPpo,
                alpha_lose,
                BASE_SEED,
            );

            let ppo = run_matrix_experiments(ppo_config, RUNS, BASE_SEED);
            let wolf = run_matrix_experiments(wolf_config, RUNS, BASE_SEED);

            let ppo_distances: Vec<f64> = ppo.iter().map(|r| r.max_nes_distance_last_n).collect();
            let wolf_distances: Vec<f64> = wolf.iter().map(|r| r.max_nes_distance_last_n).collect();

            let (ppo_mean, _) = mean_std(&ppo_distances);
            let (wolf_mean, _) = mean_std(&wolf_distances);

            eprintln!(
                "weighted MP α_LOSE={alpha_lose}: PPO mean max-dist (last {DEFAULT_LAST_N_POLICY_UPDATES}) = {ppo_mean:.4}, WoLF-PPO = {wolf_mean:.4}"
            );

            assert!(
                wolf_mean < ppo_mean,
                "expected WoLF-PPO closer to NES than PPO at α_LOSE={alpha_lose} (WoLF={wolf_mean:.4}, PPO={ppo_mean:.4})"
            );
        }
    }
}
