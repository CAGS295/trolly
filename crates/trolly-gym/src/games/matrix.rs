//! Two-player zero-sum matrix games from Ratcliffe et al. (IEEE CoG 2019).

/// Known matrix-game variants used in the WoLF-PPO paper.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MatrixGameKind {
    /// Standard Matching Pennies; NES is uniform random (`P(H) = 0.5`).
    MatchingPennies,
    /// Weighted Matching Pennies (Table IIa); NES `P(H) = 0.4`.
    WeightedMatchingPennies,
    /// Standard Rock–Paper–Scissors; NES is uniform random.
    RockPaperScissors,
    /// Weighted Rock–Paper–Scissors (Table IIb); NES `P(R)=0.2`, `P(P)=0.4`.
    WeightedRockPaperScissors,
}

impl MatrixGameKind {
    /// All variants exercised by the validation harness.
    pub fn all() -> &'static [MatrixGameKind] {
        &[
            MatrixGameKind::MatchingPennies,
            MatrixGameKind::WeightedMatchingPennies,
            MatrixGameKind::RockPaperScissors,
            MatrixGameKind::WeightedRockPaperScissors,
        ]
    }

    /// Human-readable label for logging and tests.
    pub fn name(self) -> &'static str {
        match self {
            Self::MatchingPennies => "matching_pennies",
            Self::WeightedMatchingPennies => "weighted_matching_pennies",
            Self::RockPaperScissors => "rock_paper_scissors",
            Self::WeightedRockPaperScissors => "weighted_rock_paper_scissors",
        }
    }
}

/// Payoff matrix for a two-player zero-sum normal-form game.
///
/// `payoffs[row_action][col_action]` is the row player's payoff; the column
/// player receives the negated value.
#[derive(Debug, Clone)]
pub struct MatrixGame {
    kind: MatrixGameKind,
    /// Row-player payoffs indexed by `[row_action][col_action]`.
    payoffs: Vec<Vec<f64>>,
    /// Known Nash equilibrium action probabilities (same ordering as actions).
    nes: Vec<f64>,
}

impl MatrixGame {
    /// Build the game definition for `kind`.
    pub fn new(kind: MatrixGameKind) -> Self {
        let (payoffs, nes) = match kind {
            MatrixGameKind::MatchingPennies => (
                vec![vec![1.0, -1.0], vec![-1.0, 1.0]],
                vec![0.5, 0.5],
            ),
            MatrixGameKind::WeightedMatchingPennies => (
                vec![vec![2.0, -1.0], vec![-1.0, 1.0]],
                vec![0.4, 0.6],
            ),
            MatrixGameKind::RockPaperScissors => (
                vec![
                    vec![0.0, -1.0, 1.0],
                    vec![1.0, 0.0, -1.0],
                    vec![-1.0, 1.0, 0.0],
                ],
                vec![1.0 / 3.0, 1.0 / 3.0, 1.0 / 3.0],
            ),
            MatrixGameKind::WeightedRockPaperScissors => (
                vec![
                    vec![0.0, -1.0, 1.0],
                    vec![2.0, 0.0, -1.0],
                    vec![-2.0, 1.0, 0.0],
                ],
                vec![0.2, 0.4, 0.4],
            ),
        };

        Self {
            kind,
            payoffs,
            nes,
        }
    }

    pub fn kind(&self) -> MatrixGameKind {
        self.kind
    }

    pub fn num_actions(&self) -> usize {
        self.payoffs.len()
    }

    /// Nash equilibrium action probabilities (known analytically for these games).
    pub fn nes_probabilities(&self) -> &[f64] {
        &self.nes
    }

    /// Row-player payoff for a simultaneous action pair.
    pub fn row_payoff(&self, row_action: usize, col_action: usize) -> f64 {
        self.payoffs[row_action][col_action]
    }

    /// Column-player payoff (zero-sum complement).
    pub fn col_payoff(&self, row_action: usize, col_action: usize) -> f64 {
        -self.row_payoff(row_action, col_action)
    }

    /// Constant state dimension for the stateless matrix-game MLP policy.
    pub fn obs_dim(&self) -> i64 {
        1
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn weighted_matching_pennies_nes_is_point_four_heads() {
        let game = MatrixGame::new(MatrixGameKind::WeightedMatchingPennies);
        let nes = game.nes_probabilities();
        assert_eq!(nes.len(), 2);
        assert!((nes[0] - 0.4).abs() < 1e-9);
        assert!((nes[1] - 0.6).abs() < 1e-9);
    }

    #[test]
    fn weighted_rps_nes_matches_table_iib() {
        let game = MatrixGame::new(MatrixGameKind::WeightedRockPaperScissors);
        let nes = game.nes_probabilities();
        assert_eq!(nes.len(), 3);
        assert!((nes[0] - 0.2).abs() < 1e-9);
        assert!((nes[1] - 0.4).abs() < 1e-9);
        assert!((nes[2] - 0.4).abs() < 1e-9);
    }

    #[test]
    fn standard_games_are_zero_sum() {
        for kind in MatrixGameKind::all() {
            let game = MatrixGame::new(*kind);
            for r in 0..game.num_actions() {
                for c in 0..game.num_actions() {
                    let row = game.row_payoff(r, c);
                    let col = game.col_payoff(r, c);
                    assert!((row + col).abs() < 1e-9, "{kind:?} ({r},{c})");
                }
            }
        }
    }
}
