#![cfg(feature = "ort")]

use std::cell::Cell;

use trolly_gym::{
    run_offline_policy_harness, Action, Env, EnvConfig, OnnxGaussianMeanPolicy, OnnxPolicy,
    OnnxPolicyError, PolicyProvider,
};
use trolly_strategy::{envelope_message, DepthUpdate, PriceLevel, RecordingEgress, StreamEvent};

fn temp_file(name: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!(
        "trolly_gym_onnx_policy_{name}_{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ))
}

fn depth_message(symbol: &str) -> trolly_stream::Message {
    envelope_message(&StreamEvent::Depth(DepthUpdate {
        symbol: symbol.into(),
        bids: vec![PriceLevel {
            price: "100".into(),
            qty: "1".into(),
        }],
        asks: vec![PriceLevel {
            price: "101".into(),
            qty: "1".into(),
        }],
        update_id: Some(1),
    }))
}

#[test]
fn missing_onnx_model_reports_error_without_loading_runtime() {
    let path = temp_file("missing.onnx");
    let err = OnnxPolicy::from_model(&path, 7).unwrap_err();
    assert!(matches!(err, OnnxPolicyError::MissingModel(_)));
}

#[test]
fn missing_gaussian_onnx_model_reports_error_without_loading_runtime() {
    let path = temp_file("missing_gaussian.onnx");
    let err = OnnxGaussianMeanPolicy::from_model(&path, 40, 0.25).unwrap_err();
    assert!(matches!(err, OnnxPolicyError::MissingModel(_)));
}

#[test]
fn invalid_onnx_model_reports_load_error_or_skips_without_native_runtime() {
    let path = temp_file("invalid.onnx");
    std::fs::write(&path, b"not an onnx graph").unwrap();

    let err = OnnxPolicy::from_model(&path, 7).unwrap_err();
    let _ = std::fs::remove_file(&path);

    if err.is_runtime_unavailable() {
        eprintln!("skipping invalid-model assertion: {err}");
        return;
    }

    assert!(matches!(err, OnnxPolicyError::Load { .. }));
}

#[test]
fn offline_harness_steps_with_deterministic_policy_double_under_ort() {
    struct OneShotPolicy {
        calls: Cell<usize>,
    }

    impl PolicyProvider for OneShotPolicy {
        fn act(&self, obs: &[f32]) -> Action {
            assert_eq!(obs.len(), 7);
            self.calls.set(self.calls.get() + 1);
            Action::Buy
        }
    }

    let mut config = EnvConfig::new("BTCUSDT");
    config.window_frames = 1;
    let mut env = Env::new(config, RecordingEgress::default());
    let policy = OneShotPolicy {
        calls: Cell::new(0),
    };

    let steps = run_offline_policy_harness(&mut env, &policy, [depth_message("BTCUSDT")]).unwrap();

    assert_eq!(steps.len(), 1);
    assert_eq!(policy.calls.get(), 1);
    assert_eq!(
        env.egress().dispatched,
        vec![Action::Buy.to_outbound("BTCUSDT", "0.01", None)]
    );
}
