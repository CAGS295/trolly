//! Two-player zero-sum matrix games from Ratcliffe et al. (IEEE CoG 2019).

/// Named matrix games used in the WoLF-PPO paper.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MatrixGameKind {
    MatchingPenniesStandard,
    MatchingPenniesWeighted,
    RockPaperScissorsStandard,
    RockPaperScissorsWeighted,
}

/// Payoff matrix for row player plus known Nash equilibrium mixed strategy.
#[derive(Debug, Clone)]
pub struct MatrixGame {
    pub kind: MatrixGameKind,
    /// Row-player payoffs: `payoffs[row_action][col_action]`.
    pub payoffs: Vec<Vec<f64>>,
    /// Target NES action probabilities (same for both players in these symmetric games).
    pub nes: Vec<f64>,
}

impl MatrixGame {
    pub fn from_kind(kind: MatrixGameKind) -> Self {
        match kind {
            MatrixGameKind::MatchingPenniesStandard => Self {
                kind,
                payoffs: vec![vec![1.0, -1.0], vec![-1.0, 1.0]],
                nes: vec![0.5, 0.5],
            },
            MatrixGameKind::MatchingPenniesWeighted => Self {
                kind,
                // Table IIa: H/T payoffs (row, col).
                payoffs: vec![vec![2.0, -1.0], vec![-1.0, 1.0]],
                nes: vec![0.4, 0.6],
            },
            MatrixGameKind::RockPaperScissorsStandard => Self {
                kind,
                payoffs: vec![
                    vec![0.0, -1.0, 1.0],
                    vec![1.0, 0.0, -1.0],
                    vec![-1.0, 1.0, 0.0],
                ],
                nes: vec![1.0 / 3.0, 1.0 / 3.0, 1.0 / 3.0],
            },
            MatrixGameKind::RockPaperScissorsWeighted => Self {
                kind,
                // Table IIb: R/P/S payoffs (row, col).
                payoffs: vec![
                    vec![0.0, -1.0, 1.0],
                    vec![2.0, 0.0, -1.0],
                    vec![-2.0, 1.0, 0.0],
                ],
                // NES: P(R)=0.2, P(P)=0.4, P(S)=0.4 (paper Fig. 2c / Table I).
                nes: vec![0.2, 0.4, 0.4],
            },
        }
    }

    pub fn action_count(&self) -> usize {
        self.payoffs.len()
    }

    /// Row-player payoff for a joint action profile.
    pub fn payoff(&self, row_action: usize, col_action: usize) -> f64 {
        self.payoffs[row_action][col_action]
    }

    /// Column-player payoff (zero-sum).
    pub fn payoff_col(&self, row_action: usize, col_action: usize) -> f64 {
        -self.payoff(row_action, col_action)
    }

    /// Constant observation dimension for stateless matrix games (single dummy feature).
    pub fn obs_dim(&self) -> usize {
        1
    }
}

impl MatrixGameKind {
    pub fn all() -> [Self; 4] {
        [
            Self::MatchingPenniesStandard,
            Self::MatchingPenniesWeighted,
            Self::RockPaperScissorsStandard,
            Self::RockPaperScissorsWeighted,
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn weighted_matching_pennies_nes_is_point_four_heads() {
        let game = MatrixGame::from_kind(MatrixGameKind::MatchingPenniesWeighted);
        assert!((game.nes[0] - 0.4).abs() < 1e-12);
        assert!((game.nes[1] - 0.6).abs() < 1e-12);
    }

    #[test]
    fn weighted_rps_nes_matches_paper() {
        let game = MatrixGame::from_kind(MatrixGameKind::RockPaperScissorsWeighted);
        assert!((game.nes[0] - 0.2).abs() < 1e-12);
        assert!((game.nes[1] - 0.4).abs() < 1e-12);
        assert!((game.nes[2] - 0.4).abs() < 1e-12);
    }

    #[test]
    fn standard_games_are_uniform_nes() {
        let mp = MatrixGame::from_kind(MatrixGameKind::MatchingPenniesStandard);
        assert!((mp.nes[0] - 0.5).abs() < 1e-12);

        let rps = MatrixGame::from_kind(MatrixGameKind::RockPaperScissorsStandard);
        for p in &rps.nes {
            assert!((p - 1.0 / 3.0).abs() < 1e-12);
        }
    }
}
