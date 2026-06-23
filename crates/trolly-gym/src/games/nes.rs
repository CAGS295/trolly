//! Nash equilibrium strategies and distance metrics (paper Table I methodology).

use super::matrix::{MatrixGame, MatrixGameKind};

/// Known Nash equilibrium action probabilities for each game variant.
#[derive(Debug, Clone, PartialEq)]
pub struct NesPolicy {
    pub probabilities: Vec<f64>,
}

impl NesPolicy {
    /// NES for the given matrix game (row player's mixed strategy).
    pub fn for_game(game: &MatrixGame) -> Self {
        let probabilities = match game.kind {
            MatrixGameKind::MatchingPenniesStandard => vec![0.5, 0.5],
            MatrixGameKind::MatchingPenniesWeighted => vec![0.4, 0.6],
            MatrixGameKind::RockPaperScissorsStandard => {
                vec![1.0 / 3.0, 1.0 / 3.0, 1.0 / 3.0]
            }
            MatrixGameKind::RockPaperScissorsWeighted => vec![0.2, 0.4, 0.4],
        };
        Self { probabilities }
    }

    /// Euclidean distance between a learned policy and this NES.
    pub fn euclidean_distance(&self, learned: &[f64]) -> f64 {
        assert_eq!(
            learned.len(),
            self.probabilities.len(),
            "policy dimension mismatch"
        );
        learned
            .iter()
            .zip(self.probabilities.iter())
            .map(|(a, b)| (a - b).powi(2))
            .sum::<f64>()
            .sqrt()
    }
}

/// Maximum Euclidean distance from NES over the last `window` policy updates.
pub fn max_distance_over_last_n(distances: &[f64], window: usize) -> f64 {
    if distances.is_empty() {
        return f64::NAN;
    }
    let start = distances.len().saturating_sub(window);
    distances[start..]
        .iter()
        .copied()
        .fold(f64::NEG_INFINITY, f64::max)
}

/// Mean of per-run max distances (paper: average over 50 runs).
pub fn mean_max_distance_last_n(run_max_distances: &[f64]) -> f64 {
    if run_max_distances.is_empty() {
        return f64::NAN;
    }
    run_max_distances.iter().sum::<f64>() / run_max_distances.len() as f64
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::games::matrix::MatrixGame;

    #[test]
    fn weighted_matching_pennies_nes_is_point_four_heads() {
        let game = MatrixGame::from_kind(MatrixGameKind::MatchingPenniesWeighted);
        let nes = NesPolicy::for_game(&game);
        assert!((nes.probabilities[0] - 0.4).abs() < 1e-9);
        assert!((nes.probabilities[1] - 0.6).abs() < 1e-9);
    }

    #[test]
    fn weighted_rps_nes_matches_paper() {
        let game = MatrixGame::from_kind(MatrixGameKind::RockPaperScissorsWeighted);
        let nes = NesPolicy::for_game(&game);
        assert!((nes.probabilities[0] - 0.2).abs() < 1e-9);
        assert!((nes.probabilities[1] - 0.4).abs() < 1e-9);
        assert!((nes.probabilities[2] - 0.4).abs() < 1e-9);
    }

    #[test]
    fn max_distance_over_last_ten_updates() {
        let distances = vec![0.1, 0.5, 0.3, 0.9, 0.2];
        assert!((max_distance_over_last_n(&distances, 3) - 0.9).abs() < 1e-9);
    }
}
