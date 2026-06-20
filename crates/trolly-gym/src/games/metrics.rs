//! Euclidean distance metric to Nash Equilibrium Strategy (NES).
//!
//! Used by the self-play training harness to measure how close the learned
//! policy is to the known NES for each matrix game variant.

/// Euclidean distance between a learned policy distribution and the known NES.
///
/// Both slices must have the same length (number of actions). Panics otherwise.
///
/// # Example
///
/// ```ignore
/// // Matching Pennies weighted: NES = [0.4, 0.6]
/// let dist = euclidean_distance_to_nes(&[0.5, 0.5], &[0.4, 0.6]);
/// // dist ≈ sqrt((0.5−0.4)² + (0.5−0.6)²) = sqrt(0.02) ≈ 0.141
/// ```
pub fn euclidean_distance_to_nes(policy_probs: &[f64], nes_probs: &[f64]) -> f64 {
    assert_eq!(
        policy_probs.len(),
        nes_probs.len(),
        "policy and NES must have the same dimension"
    );
    policy_probs
        .iter()
        .zip(nes_probs.iter())
        .map(|(p, q)| (p - q).powi(2))
        .sum::<f64>()
        .sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn distance_to_self_is_zero() {
        let nes = [0.4_f64, 0.6];
        assert!(euclidean_distance_to_nes(&nes, &nes) < 1e-12);
    }

    #[test]
    fn distance_from_uniform_to_weighted_mp_nes() {
        let uniform = [0.5_f64, 0.5];
        let nes = [0.4_f64, 0.6];
        let d = euclidean_distance_to_nes(&uniform, &nes);
        assert!(d.is_finite());
        assert!(d > 0.0);
        // Expected: sqrt((0.5−0.4)² + (0.5−0.6)²) = sqrt(0.02)
        let expected = (0.02_f64).sqrt();
        assert!((d - expected).abs() < 1e-12);
    }

    #[test]
    fn distance_rps_uniform_to_weighted_nes() {
        let uniform = [1.0_f64 / 3.0, 1.0 / 3.0, 1.0 / 3.0];
        let nes = [0.2_f64, 0.4, 0.4];
        let d = euclidean_distance_to_nes(&uniform, &nes);
        assert!(d.is_finite());
        assert!(d > 0.0);
    }
}
