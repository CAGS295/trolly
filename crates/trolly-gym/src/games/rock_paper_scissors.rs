//! Rock–Paper–Scissors matrix games — standard and weighted variants.
//!
//! Both variants are from Ratcliffe et al. (2019), Table II.
//!
//! Action order throughout: R = 0, P = 1, S = 2.
//!
//! ## Standard (symmetric)
//!
//! ```text
//!      R     P     S
//! R  [ 0,   -1,    1]
//! P  [ 1,    0,   -1]
//! S  [-1,    1,    0]
//! ```
//!
//! NES: P(R) = P(P) = P(S) = 1/3.
//!
//! ## Weighted (Table IIb)
//!
//! ```text
//!      R     P     S
//! R  [ 0,   -2,    2]
//! P  [ 2,    0,   -1]
//! S  [-2,    1,    0]
//! ```
//!
//! NES: P(R) = 0.2, P(P) = 0.4, P(S) = 0.4.
//!
//! Derivation: the payoff parameters (a=2, b=2, c=1) satisfy 2b+2c = 3a and
//! a+b = 4c, which forces each player's best-response mix to be (0.2, 0.4, 0.4).

use super::matrix_game::MatrixGame;

/// Standard RPS NES: each action with equal probability 1/3.
pub const STANDARD_NES: [f64; 3] = [1.0 / 3.0, 1.0 / 3.0, 1.0 / 3.0];

/// Weighted RPS NES (Table IIb): P(R) = 0.2, P(P) = 0.4, P(S) = 0.4.
pub const WEIGHTED_NES: [f64; 3] = [0.2, 0.4, 0.4];

/// Standard Rock–Paper–Scissors payoff matrix.
///
/// Action 0 = Rock, 1 = Paper, 2 = Scissors.
pub fn rps_standard() -> MatrixGame {
    MatrixGame::new(vec![
        vec![0.0, -1.0, 1.0],
        vec![1.0, 0.0, -1.0],
        vec![-1.0, 1.0, 0.0],
    ])
}

/// Weighted Rock–Paper–Scissors payoff matrix (Table IIb, Ratcliffe et al. 2019).
///
/// Action 0 = Rock, 1 = Paper, 2 = Scissors.
/// The asymmetric payoffs shift the NES from (1/3, 1/3, 1/3) to (0.2, 0.4, 0.4).
pub fn rps_weighted() -> MatrixGame {
    MatrixGame::new(vec![
        vec![0.0, -2.0, 2.0],
        vec![2.0, 0.0, -1.0],
        vec![-2.0, 1.0, 0.0],
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verify the weighted NES by checking that the row player is indifferent
    /// across all three actions when the column player plays q* = (0.2, 0.4, 0.4).
    #[test]
    fn weighted_nes_row_indifference() {
        let game = rps_weighted();
        let q_star = WEIGHTED_NES;
        let evs: Vec<f64> = game
            .payoff
            .iter()
            .map(|row| {
                row.iter()
                    .zip(q_star.iter())
                    .map(|(a, q)| a * q)
                    .sum::<f64>()
            })
            .collect();
        let max_ev = evs.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
        let min_ev = evs.iter().cloned().fold(f64::INFINITY, f64::min);
        assert!(
            (max_ev - min_ev).abs() < 1e-12,
            "NES indifference: EVs should be equal, got {evs:?}"
        );
    }

    #[test]
    fn nes_sum_to_one() {
        assert!((STANDARD_NES.iter().sum::<f64>() - 1.0).abs() < 1e-12);
        assert!((WEIGHTED_NES.iter().sum::<f64>() - 1.0).abs() < 1e-12);
    }
}
