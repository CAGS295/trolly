//! Two-player zero-sum matrix game representation.
//!
//! `MatrixGame` stores the row player's payoff matrix; the column player's
//! payoff is the negation (zero-sum). All training helpers treat P1 as the
//! row player.

/// Two-player zero-sum normal-form matrix game.
///
/// `payoff[i][j]` is the **row player's** payoff when the row player takes
/// action `i` and the column player takes action `j`. The column player's
/// payoff is `−payoff[i][j]` (zero-sum).
#[derive(Debug, Clone)]
pub struct MatrixGame {
    /// Payoff matrix `[num_row_actions][num_col_actions]`.
    pub payoff: Vec<Vec<f64>>,
    /// Number of discrete actions available to the row player.
    pub num_row_actions: usize,
    /// Number of discrete actions available to the column player.
    pub num_col_actions: usize,
}

impl MatrixGame {
    /// Create a new game from a row-player payoff table.
    ///
    /// Panics if the table is empty or rows have inconsistent length.
    pub fn new(payoff: Vec<Vec<f64>>) -> Self {
        assert!(!payoff.is_empty(), "payoff matrix must be non-empty");
        let num_col_actions = payoff[0].len();
        assert!(
            num_col_actions > 0,
            "payoff matrix must have at least one column"
        );
        for row in &payoff {
            assert_eq!(
                row.len(),
                num_col_actions,
                "all rows must have the same length"
            );
        }
        let num_row_actions = payoff.len();
        Self {
            payoff,
            num_row_actions,
            num_col_actions,
        }
    }

    /// Row player's payoff for the given action pair.
    ///
    /// Panics if either index is out of bounds.
    #[inline]
    pub fn payoff_for(&self, row_action: usize, col_action: usize) -> f64 {
        self.payoff[row_action][col_action]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matching_pennies_payoffs() {
        let game = MatrixGame::new(vec![vec![1.0, -1.0], vec![-1.0, 1.0]]);
        assert_eq!(game.num_row_actions, 2);
        assert_eq!(game.num_col_actions, 2);
        assert_eq!(game.payoff_for(0, 0), 1.0);
        assert_eq!(game.payoff_for(0, 1), -1.0);
        assert_eq!(game.payoff_for(1, 0), -1.0);
        assert_eq!(game.payoff_for(1, 1), 1.0);
    }

    #[test]
    #[should_panic]
    fn empty_payoff_panics() {
        MatrixGame::new(vec![]);
    }
}
