//! NES distance metrics (paper Table I methodology).

/// Euclidean distance between learned and target mixed strategies.
pub fn euclidean_distance(learned: &[f64], target: &[f64]) -> f64 {
    assert_eq!(
        learned.len(),
        target.len(),
        "policy vectors must have equal length"
    );
    learned
        .iter()
        .zip(target)
        .map(|(a, b)| (a - b).powi(2))
        .sum::<f64>()
        .sqrt()
}

/// Maximum distance over the last `n` recorded policy snapshots (per run).
pub fn max_distance_last_n(distances: &[f64], n: usize) -> f64 {
    if distances.is_empty() {
        return 0.0;
    }
    let start = distances.len().saturating_sub(n);
    distances[start..]
        .iter()
        .copied()
        .fold(0.0_f64, f64::max)
}

/// Mean and sample standard deviation over independent runs.
pub fn mean_std(values: &[f64]) -> (f64, f64) {
    if values.is_empty() {
        return (0.0, 0.0);
    }
    let mean = values.iter().sum::<f64>() / values.len() as f64;
    if values.len() == 1 {
        return (mean, 0.0);
    }
    let var = values
        .iter()
        .map(|v| (v - mean).powi(2))
        .sum::<f64>()
        / (values.len() - 1) as f64;
    (mean, var.sqrt())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn euclidean_distance_matches_l2_norm() {
        let learned = [0.5, 0.5];
        let target = [0.4, 0.6];
        let expected = ((0.1_f64).powi(2) + (0.1_f64).powi(2)).sqrt();
        assert!((euclidean_distance(&learned, &target) - expected).abs() < 1e-12);
    }

    #[test]
    fn max_distance_last_n_takes_tail_maximum() {
        let distances = vec![0.1, 0.5, 0.2, 0.9, 0.3];
        assert!((max_distance_last_n(&distances, 3) - 0.9).abs() < 1e-12);
    }
}
