//! Matrix-game validation harness (WP-019, requires `--features torch`).

#![cfg(feature = "torch")]

use trolly_gym::games::{
    run_benchmark, run_self_play, MatrixExperimentConfig, MatrixGameKind, MatrixTrainerKind,
};

#[test]
fn smoke_wolf_ppo_weighted_matching_pennies_finite_nes_distance() {
    let config = MatrixExperimentConfig::smoke(MatrixGameKind::MatchingPenniesWeighted, 42);
    let result = run_self_play(&config);

    assert_eq!(result.distances.len(), config.num_updates);
    assert!(result.distances.iter().all(|d| d.is_finite()));
    assert!(result.max_distance_last_n.is_finite());
}

#[test]
fn nes_policies_match_paper_for_all_games() {
    use trolly_gym::games::{MatrixGame, NesPolicy};

    for kind in [
        MatrixGameKind::MatchingPenniesStandard,
        MatrixGameKind::MatchingPenniesWeighted,
        MatrixGameKind::RockPaperScissorsStandard,
        MatrixGameKind::RockPaperScissorsWeighted,
    ] {
        let game = MatrixGame::from_kind(kind);
        let nes = NesPolicy::for_game(&game);
        let sum: f64 = nes.probabilities.iter().sum();
        assert!((sum - 1.0).abs() < 1e-9, "NES must sum to 1 for {kind:?}");
        assert!(nes.probabilities.iter().all(|p| *p >= 0.0));
    }
}

#[test]
#[ignore = "extended benchmark: run locally with `cargo test -p trolly-gym --features torch matrix_games -- --ignored --test-threads=1`"]
fn benchmark_wolf_ppo_closer_to_nes_than_ppo_on_weighted_matching_pennies() {
    // Paper Table I: WoLF-PPO stays closer to NES than PPO on weighted matching pennies
    // at α_LOSE ∈ {0.1, 0.01}. Uses fewer runs than the paper's 50 for CI practicality.
    const NUM_RUNS: usize = 10;
    const NUM_UPDATES: usize = 400;

    for alpha_lose in [0.1_f64, 0.01] {
        let mut ppo_config =
            MatrixExperimentConfig::paper_default(
                MatrixGameKind::MatchingPenniesWeighted,
                MatrixTrainerKind::Ppo,
                alpha_lose,
            );
        ppo_config.num_updates = NUM_UPDATES;
        ppo_config.seed = 7;

        let mut wolf_config = ppo_config.clone();
        wolf_config.trainer = MatrixTrainerKind::WolfPpo;

        let ppo = run_benchmark(&ppo_config, NUM_RUNS);
        let wolf = run_benchmark(&wolf_config, NUM_RUNS);

        assert!(
            wolf.mean_max_distance < ppo.mean_max_distance,
            "α_LOSE={alpha_lose}: expected WoLF-PPO mean max NES distance ({}) \
             < PPO ({}) on weighted matching pennies",
            wolf.mean_max_distance,
            ppo.mean_max_distance
        );
    }
}
