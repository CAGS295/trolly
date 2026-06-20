//! Matching Pennies matrix games — standard and weighted variants.
//!
//! Both variants are from Ratcliffe et al. (2019), Table II.
//!
//! ## Standard (symmetric)
//!
//! ```text
//!      H     T
//! H  [ 1,   -1]
//! T  [-1,    1]
//! ```
//!
//! NES: P(H) = P(T) = 0.5 (uniform over both actions).
//!
//! ## Weighted (Table IIa)
//!
//! ```text
//!      H     T
//! H  [ 2,   -1]
//! T  [-1,    1]
//! ```
//!
//! NES: P(H) = 0.4, P(T) = 0.6.
//!
//! The asymmetric payoff on (H, H) shifts the equilibrium: each player is
//! indifferent between H and T only when the opponent plays H with probability 0.4.
//!
//! Derivation:
//!   Row indifferent ⟺ 2·q*(H) − q*(T) = −q*(H) + q*(T) ⟹ q*(H) = 2/(w+3) = 0.4 (w=2).

use super::matrix_game::MatrixGame;

/// Standard Matching Pennies NES: each action with probability 0.5.
pub const STANDARD_NES: [f64; 2] = [0.5, 0.5];

/// Weighted Matching Pennies NES: P(H) = 0.4, P(T) = 0.6 (Table IIa).
pub const WEIGHTED_NES: [f64; 2] = [0.4, 0.6];

/// Standard Matching Pennies payoff matrix.
///
/// Action 0 = Heads, action 1 = Tails.
/// Row player wins (+1) when both match; loses (−1) when they differ.
pub fn matching_pennies_standard() -> MatrixGame {
    MatrixGame::new(vec![vec![1.0, -1.0], vec![-1.0, 1.0]])
}

/// Weighted Matching Pennies payoff matrix (Table IIa, Ratcliffe et al. 2019).
///
/// Action 0 = Heads, action 1 = Tails.
/// The (H, H) payoff is 2 instead of 1, biasing the NES to P(H) = 0.4.
pub fn matching_pennies_weighted() -> MatrixGame {
    MatrixGame::new(vec![vec![2.0, -1.0], vec![-1.0, 1.0]])
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verify the weighted NES by checking that the row player is indifferent
    /// between H and T when the column player plays q* = (0.4, 0.6).
    #[test]
    fn weighted_nes_row_indifference() {
        let game = matching_pennies_weighted();
        let q_star = WEIGHTED_NES;
        let ev_h: f64 = game.payoff[0]
            .iter()
            .zip(q_star.iter())
            .map(|(a, q)| a * q)
            .sum();
        let ev_t: f64 = game.payoff[1]
            .iter()
            .zip(q_star.iter())
            .map(|(a, q)| a * q)
            .sum();
        assert!(
            (ev_h - ev_t).abs() < 1e-12,
            "NES indifference: EV(H)={ev_h}, EV(T)={ev_t}"
        );
    }

    #[test]
    fn nes_sum_to_one() {
        assert!((STANDARD_NES.iter().sum::<f64>() - 1.0).abs() < 1e-12);
        assert!((WEIGHTED_NES.iter().sum::<f64>() - 1.0).abs() < 1e-12);
    }
}
