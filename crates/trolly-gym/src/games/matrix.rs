//! Two-player zero-sum matrix games from Ratcliffe et al. (IEEE CoG 2019).

/// Which matrix game variant to run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MatrixGameKind {
    /// Standard matching pennies — NES is uniform random.
    MatchingPenniesStandard,
    /// Weighted matching pennies (Table IIa) — NES `P(H) = 0.4`.
    MatchingPenniesWeighted,
    /// Standard rock–paper–scissors — NES is uniform random.
    RockPaperScissorsStandard,
    /// Weighted rock–paper–scissors (Table IIb) — NES `P(R)=0.2`, `P(P)=0.4`.
    RockPaperScissorsWeighted,
}

/// Row-player payoff matrix for a two-player zero-sum simultaneous game.
#[derive(Debug, Clone)]
pub struct MatrixGame {
    pub kind: MatrixGameKind,
    /// Row-player payoffs: `payoffs[row_action][col_action]`.
    pub payoffs: Vec<Vec<f64>>,
    pub action_names: Vec<&'static str>,
}

impl MatrixGame {
    /// Build a predefined game from the WoLF-PPO paper.
    pub fn from_kind(kind: MatrixGameKind) -> Self {
        match kind {
            MatrixGameKind::MatchingPenniesStandard => Self {
                kind,
                payoffs: vec![vec![1.0, -1.0], vec![-1.0, 1.0]],
                action_names: vec!["H", "T"],
            },
            MatrixGameKind::MatchingPenniesWeighted => Self {
                kind,
                // Table IIa — row player receives the first component of each tuple.
                payoffs: vec![vec![2.0, -1.0], vec![-1.0, 1.0]],
                action_names: vec!["H", "T"],
            },
            MatrixGameKind::RockPaperScissorsStandard => Self {
                kind,
                payoffs: vec![
                    vec![0.0, -1.0, 1.0],
                    vec![1.0, 0.0, -1.0],
                    vec![-1.0, 1.0, 0.0],
                ],
                action_names: vec!["R", "P", "S"],
            },
            MatrixGameKind::RockPaperScissorsWeighted => Self {
                kind,
                // Table IIb — row player payoffs.
                payoffs: vec![
                    vec![0.0, -1.0, 1.0],
                    vec![2.0, 0.0, -1.0],
                    vec![-2.0, 1.0, 0.0],
                ],
                action_names: vec!["R", "P", "S"],
            },
        }
    }

    /// Number of discrete actions per player.
    pub fn n_actions(&self) -> usize {
        self.payoffs.len()
    }

    /// Row-player reward for `(row_action, col_action)`.
    pub fn row_payoff(&self, row: i64, col: i64) -> f64 {
        self.payoffs[row as usize][col as usize]
    }

    /// Column-player reward (zero-sum negation of row payoff).
    pub fn col_payoff(&self, row: i64, col: i64) -> f64 {
        -self.row_payoff(row, col)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn weighted_matching_pennies_payoffs_match_table_iia() {
        let game = MatrixGame::from_kind(MatrixGameKind::MatchingPenniesWeighted);
        assert_eq!(game.row_payoff(0, 0), 2.0);
        assert_eq!(game.row_payoff(0, 1), -1.0);
        assert_eq!(game.row_payoff(1, 0), -1.0);
        assert_eq!(game.row_payoff(1, 1), 1.0);
    }

    #[test]
    fn weighted_rps_payoffs_match_table_iib() {
        let game = MatrixGame::from_kind(MatrixGameKind::RockPaperScissorsWeighted);
        assert_eq!(game.row_payoff(0, 1), -1.0);
        assert_eq!(game.row_payoff(1, 0), 2.0);
        assert_eq!(game.row_payoff(2, 0), -2.0);
    }
}
