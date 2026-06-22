//! NES distance metrics (Ratcliffe et al. 2019 Table I methodology).

/// Euclidean distance between learned and Nash equilibrium action probabilities.
pub fn euclidean_distance(learned: &[f64], nes: &[f64]) -> f64 {
    assert_eq!(
        learned.len(),
        nes.len(),
        "policy and NES must have the same action count"
    );
    learned
        .iter()
        .zip(nes.iter())
        .map(|(p, q)| (p - q).powi(2))
        .sum::<f64>()
        .sqrt()
}

/// Maximum distance over the last `last_n` recorded policy updates (paper Table I).
pub fn max_distance_over_last(distances: &[f64], last_n: usize) -> f64 {
    if distances.is_empty() {
        return f64::NAN;
    }
    let start = distances.len().saturating_sub(last_n);
    distances[start..]
        .iter()
        .copied()
        .fold(f64::NEG_INFINITY, f64::max)
}

/// Mean of per-run maxima (each run already reduced via [`max_distance_over_last`]).
pub fn mean_of_run_maxima(run_maxima: &[f64]) -> f64 {
    if run_maxima.is_empty() {
        return f64::NAN;
    }
    run_maxima.iter().sum::<f64>() / run_maxima.len() as f64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn euclidean_distance_at_nes_is_zero() {
        let nes = [0.4, 0.6];
        assert!((euclidean_distance(&nes, &nes) - 0.0).abs() < 1e-12);
    }

    #[test]
    fn max_distance_over_last_ten_picks_worst_in_window() {
        let distances = vec![0.1, 0.05, 0.2, 0.15, 0.3, 0.12, 0.08, 0.4, 0.11, 0.09, 0.07];
        assert!((max_distance_over_last(&distances, 10) - 0.4).abs() < 1e-9);
    }
}
