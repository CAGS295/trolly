//! libtorch.rs inference hooks (requires `--features torch` and a libtorch install).

use tch::{Device, Kind, Tensor};

/// Wrap a flattened observation window as a CPU float tensor for model input.
pub fn observation_tensor(flat: &[f32]) -> Tensor {
    Tensor::from_slice(flat).to_kind(Kind::Float)
}

/// Stack row-major observation rows into a batch tensor `[batch, obs_dim]`.
pub fn batch_observations(rows: &[&[f32]]) -> Tensor {
    if rows.is_empty() {
        return Tensor::zeros([0, 0], (Kind::Float, Device::Cpu));
    }
    let obs_dim = rows[0].len() as i64;
    let batch = rows.len() as i64;
    let flat: Vec<f32> = rows.iter().flat_map(|r| r.iter().copied()).collect();
    Tensor::from_slice(&flat)
        .to_kind(Kind::Float)
        .reshape([batch, obs_dim])
}

/// Stub inference hook: identity map until a trained policy is wired in.
pub fn stub_forward(input: &Tensor) -> Tensor {
    input.shallow_clone()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn observation_tensor_roundtrip() {
        let data = vec![1.0_f32, 2.0, 3.0];
        let tensor = observation_tensor(&data);
        let out: Vec<f32> = Vec::try_from(tensor).unwrap();
        assert_eq!(out, data);
    }

    #[test]
    fn batch_observations_shape() {
        let a = [1.0_f32, 2.0];
        let b = [3.0_f32, 4.0];
        let batch = batch_observations(&[&a, &b]);
        assert_eq!(batch.size(), vec![2, 2]);
    }
}
