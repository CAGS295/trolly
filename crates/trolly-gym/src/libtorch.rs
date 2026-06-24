//! libtorch.rs inference hooks (requires `--features torch` and a libtorch install).

use tch::{Kind, Tensor};

/// Wrap a flattened observation window as a CPU float tensor for model input.
pub fn observation_tensor(flat: &[f32]) -> Tensor {
    Tensor::from_slice(flat).to_kind(Kind::Float)
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
        let out: Vec<f32> = Vec::<f32>::try_from(&tensor).unwrap();
        assert_eq!(out, data);
    }
}
