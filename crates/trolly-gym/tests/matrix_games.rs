//! Matrix-game validation harness — WP-019.
//!
//! # Test structure
//!
//! - **Always-run smoke tests** (top level, no `tch` dependency): verify the
//!   distance metric logic and NES constants using inline arithmetic only.
//!   These compile and run with the default feature set.
//!
//! - **Torch-gated tests** (`#[cfg(feature = "torch")]`): verify that the
//!   PPO and WoLF-PPO training steps complete and produce finite NES distances.
//!   Requires `--features torch` (and a working libtorch install).
//!
//! - **`#[ignore]` extended benchmark**: reproduce the paper trend that
//!   WoLF-PPO converges closer to the NES than standard PPO on weighted
//!   Matching Pennies.
//!
//! # Running the extended benchmark
//!
//! ```bash
//! export LIBTORCH_USE_PYTORCH=1
//! export LIBTORCH_BYPASS_VERSION_CHECK=1
//! export RUSTFLAGS="-L /usr/lib/gcc/x86_64-linux-gnu/13"
//! cargo test -p trolly-gym --features torch -- --include-ignored
//! ```

// ── Always-run smoke tests (no tch) ──────────────────────────────────────────

/// Local Euclidean distance helper — mirrors `games::metrics::euclidean_distance_to_nes`.
/// Defined inline so these tests compile without the `torch` feature.
fn euclidean_distance(a: &[f64], b: &[f64]) -> f64 {
    a.iter()
        .zip(b.iter())
        .map(|(x, y)| (x - y).powi(2))
        .sum::<f64>()
        .sqrt()
}

#[test]
fn smoke_distance_metric_finite_and_positive() {
    let nes = [0.4_f64, 0.6];
    let uniform = [0.5_f64, 0.5];
    let dist = euclidean_distance(&uniform, &nes);
    assert!(dist.is_finite(), "distance must be finite");
    assert!(dist > 0.0, "uniform ≠ NES → distance > 0");
    let self_dist = euclidean_distance(&nes, &nes);
    assert!(self_dist < 1e-12, "distance to self must be ~0");
}

#[test]
fn matching_pennies_weighted_nes_sums_to_one() {
    // P(H) = 0.4, P(T) = 0.6  (Table IIa)
    let nes = [0.4_f64, 0.6];
    let sum: f64 = nes.iter().sum();
    assert!((sum - 1.0).abs() < 1e-12);
}

#[test]
fn rps_weighted_nes_sums_to_one() {
    // P(R) = 0.2, P(P) = 0.4, P(S) = 0.4  (Table IIb)
    let nes = [0.2_f64, 0.4, 0.4];
    let sum: f64 = nes.iter().sum();
    assert!((sum - 1.0).abs() < 1e-12);
}

/// Verify the weighted Matching Pennies NES analytically.
///
/// Payoff matrix A = [[2, -1], [-1, 1]].
/// Row player is indifferent at q* = (0.4, 0.6):
///   EV(H) = 2·0.4 + (−1)·0.6 = 0.2
///   EV(T) = (−1)·0.4 + 1·0.6 = 0.2  ← equal ✓
#[test]
fn weighted_matching_pennies_nes_verified() {
    let payoff = [[2.0_f64, -1.0], [-1.0, 1.0]];
    let q_star = [0.4_f64, 0.6];
    let ev_h: f64 = payoff[0].iter().zip(q_star.iter()).map(|(a, q)| a * q).sum();
    let ev_t: f64 = payoff[1].iter().zip(q_star.iter()).map(|(a, q)| a * q).sum();
    assert!(
        (ev_h - ev_t).abs() < 1e-12,
        "NES indifference: EV(H)={ev_h:.4}, EV(T)={ev_t:.4}"
    );
}

/// Verify the weighted RPS NES analytically.
///
/// Payoff matrix (row R=0, P=1, S=2) at q* = (0.2, 0.4, 0.4):
///   EV(R) = 0·0.2 + (−2)·0.4 + 2·0.4 = 0
///   EV(P) = 2·0.2 + 0·0.4 + (−1)·0.4 = 0
///   EV(S) = (−2)·0.2 + 1·0.4 + 0·0.4 = 0  ← all equal ✓
#[test]
fn weighted_rps_nes_verified() {
    let payoff = [
        [0.0_f64, -2.0, 2.0],
        [2.0, 0.0, -1.0],
        [-2.0, 1.0, 0.0],
    ];
    let q_star = [0.2_f64, 0.4, 0.4];
    let evs: Vec<f64> = payoff
        .iter()
        .map(|row| row.iter().zip(q_star.iter()).map(|(a, q)| a * q).sum())
        .collect();
    let max_ev = evs.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
    let min_ev = evs.iter().cloned().fold(f64::INFINITY, f64::min);
    assert!(
        (max_ev - min_ev).abs() < 1e-12,
        "NES indifference: EVs should all be equal, got {evs:?}"
    );
}

// ── Torch-gated tests ─────────────────────────────────────────────────────────

#[cfg(feature = "torch")]
mod torch_tests {
    use trolly_gym::games::{
        matching_pennies::{self, matching_pennies_weighted},
        rock_paper_scissors::{self, rps_weighted},
        run_ppo_self_play, run_wolf_ppo_self_play, SelfPlayConfig,
    };
    use trolly_gym::ppo::{PpoConfig, WolfPpoConfig};

    fn short_config() -> SelfPlayConfig {
        SelfPlayConfig {
            num_updates: 10,
            batch_size: 16,
            ppo_config: PpoConfig::default(),
        }
    }

    /// Smoke test: WoLF-PPO on weighted Matching Pennies completes and produces
    /// a finite NES distance — proves the training step works end-to-end.
    #[test]
    fn wolf_ppo_weighted_mp_training_step_finite() {
        let game = matching_pennies_weighted();
        let nes = &matching_pennies::WEIGHTED_NES;
        let result =
            run_wolf_ppo_self_play(&game, nes, short_config(), WolfPpoConfig::default());
        assert!(
            result.max_distance_last_10.is_finite(),
            "WoLF-PPO max NES distance must be finite, got {}",
            result.max_distance_last_10
        );
        assert!(
            result.final_policy_probs.iter().all(|&p| p.is_finite()),
            "all final policy probs must be finite: {:?}",
            result.final_policy_probs
        );
        let prob_sum: f64 = result.final_policy_probs.iter().sum();
        assert!(
            (prob_sum - 1.0).abs() < 1e-5,
            "final policy must sum to 1.0, got {prob_sum}"
        );
    }

    /// Smoke test: WoLF-PPO on standard Matching Pennies.
    #[test]
    fn wolf_ppo_standard_mp_training_step_finite() {
        use trolly_gym::games::matching_pennies::{matching_pennies_standard, STANDARD_NES};
        let game = matching_pennies_standard();
        let nes = &STANDARD_NES;
        let result =
            run_wolf_ppo_self_play(&game, nes, short_config(), WolfPpoConfig::default());
        assert!(result.max_distance_last_10.is_finite());
    }

    /// Smoke test: PPO on weighted RPS completes and produces a finite distance.
    #[test]
    fn ppo_weighted_rps_training_step_finite() {
        let game = rps_weighted();
        let nes = &rock_paper_scissors::WEIGHTED_NES;
        let result = run_ppo_self_play(&game, nes, short_config());
        assert!(
            result.max_distance_last_10.is_finite(),
            "PPO max NES distance must be finite, got {}",
            result.max_distance_last_10
        );
    }

    /// Smoke test: WoLF-PPO on weighted RPS.
    #[test]
    fn wolf_ppo_weighted_rps_training_step_finite() {
        let game = rps_weighted();
        let nes = &rock_paper_scissors::WEIGHTED_NES;
        let result =
            run_wolf_ppo_self_play(&game, nes, short_config(), WolfPpoConfig::default());
        assert!(result.max_distance_last_10.is_finite());
    }

    /// Distances are recorded for every update step.
    #[test]
    fn distances_per_update_length_matches_num_updates() {
        let game = matching_pennies_weighted();
        let nes = &matching_pennies::WEIGHTED_NES;
        let config = SelfPlayConfig {
            num_updates: 7,
            batch_size: 8,
            ppo_config: PpoConfig::default(),
        };
        let result = run_wolf_ppo_self_play(&game, nes, config, WolfPpoConfig::default());
        assert_eq!(
            result.distances_per_update.len(),
            7,
            "one distance per update"
        );
        for (i, &d) in result.distances_per_update.iter().enumerate() {
            assert!(d.is_finite(), "distance at step {i} is not finite: {d}");
        }
    }

    // ── Extended benchmark (ignored in CI) ───────────────────────────────────

    /// Reproduce the WoLF-PPO paper trend: WoLF-PPO converges closer to the NES
    /// than standard PPO on weighted Matching Pennies across multiple seeds.
    ///
    /// # What this tests
    ///
    /// For `α_LOSE ∈ {0.1, 0.01}`, the mean max-distance of WoLF-PPO should be
    /// no worse than PPO after 200 updates (paper Table I).
    ///
    /// # Running locally
    ///
    /// ```bash
    /// export LIBTORCH_USE_PYTORCH=1
    /// export LIBTORCH_BYPASS_VERSION_CHECK=1
    /// export RUSTFLAGS="-L /usr/lib/gcc/x86_64-linux-gnu/13"
    /// cargo test -p trolly-gym --features torch -- --include-ignored \
    ///     benchmark_wolf_ppo_closer_to_nes_weighted_matching_pennies
    /// ```
    ///
    /// Expected output: `WoLF-PPO (0.1)` and/or `WoLF-PPO (0.01)` mean
    /// max-distance ≤ PPO mean max-distance.
    #[test]
    #[ignore]
    fn benchmark_wolf_ppo_closer_to_nes_weighted_matching_pennies() {
        use trolly_gym::games::matching_pennies::{matching_pennies_weighted, WEIGHTED_NES};

        const NUM_SEEDS: usize = 10;
        const NUM_UPDATES: usize = 200;
        const BATCH_SIZE: usize = 64;

        let game = matching_pennies_weighted();
        let nes = &WEIGHTED_NES;

        let mut ppo_dists = Vec::with_capacity(NUM_SEEDS);
        let mut wolf_dists_01 = Vec::with_capacity(NUM_SEEDS);
        let mut wolf_dists_001 = Vec::with_capacity(NUM_SEEDS);

        for seed_idx in 0..NUM_SEEDS {
            let base_cfg = || SelfPlayConfig {
                num_updates: NUM_UPDATES,
                batch_size: BATCH_SIZE,
                ppo_config: PpoConfig::default(),
            };

            let ppo_result = run_ppo_self_play(&game, nes, base_cfg());
            ppo_dists.push(ppo_result.max_distance_last_10);

            let wolf_result_01 = run_wolf_ppo_self_play(
                &game,
                nes,
                base_cfg(),
                WolfPpoConfig::default().with_alpha_lose(0.1),
            );
            wolf_dists_01.push(wolf_result_01.max_distance_last_10);

            let wolf_result_001 = run_wolf_ppo_self_play(
                &game,
                nes,
                base_cfg(),
                WolfPpoConfig::default().with_alpha_lose(0.01),
            );
            wolf_dists_001.push(wolf_result_001.max_distance_last_10);

            println!(
                "seed {seed_idx:2}: PPO={:.4}  WoLF-0.1={:.4}  WoLF-0.01={:.4}",
                ppo_result.max_distance_last_10,
                wolf_result_01.max_distance_last_10,
                wolf_result_001.max_distance_last_10,
            );
        }

        let mean = |v: &[f64]| v.iter().sum::<f64>() / v.len() as f64;
        let ppo_mean = mean(&ppo_dists);
        let wolf_01_mean = mean(&wolf_dists_01);
        let wolf_001_mean = mean(&wolf_dists_001);

        println!();
        println!("=== Benchmark: Weighted Matching Pennies  NES = [0.4, 0.6] ===");
        println!("PPO            mean max-distance: {ppo_mean:.4}");
        println!("WoLF-PPO (0.1) mean max-distance: {wolf_01_mean:.4}");
        println!("WoLF-PPO(0.01) mean max-distance: {wolf_001_mean:.4}");
        println!();

        // At least one WoLF-PPO variant must trend closer to NES than plain PPO.
        assert!(
            wolf_01_mean <= ppo_mean || wolf_001_mean <= ppo_mean,
            "Expected WoLF-PPO closer to NES than PPO on weighted MP:\n\
             PPO={ppo_mean:.4}, WoLF-0.1={wolf_01_mean:.4}, WoLF-0.01={wolf_001_mean:.4}"
        );
    }
}
