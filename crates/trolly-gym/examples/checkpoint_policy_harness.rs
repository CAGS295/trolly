//! Offline checkpoint-or-hold policy harness over injected stream observations.
//!
//! Default builds use `HoldPolicy`. Torch builds load `latest.safetensors` from
//! `CHECKPOINT_DIR` when set. ONNX Runtime builds load `ONNX_MODEL_PATH` when
//! set. All paths feed synthetic depth envelopes through `Env`.
//!
//! ```bash
//! cargo run -p trolly-gym --example checkpoint_policy_harness
//!
//! export LIBTORCH_USE_PYTORCH=1
//! export CHECKPOINT_DIR=checkpoints/microstructure_train/mlp
//! cargo run -p trolly-gym --features torch --example checkpoint_policy_harness
//!
//! export ONNX_MODEL_PATH=checkpoints/microstructure_train/policy.onnx
//! cargo run -p trolly-gym --features ort --example checkpoint_policy_harness
//! ```

use trolly_gym::{run_offline_policy_harness, CheckpointOrHoldPolicy, Env, EnvConfig};
use trolly_strategy::{envelope_message, DepthUpdate, PriceLevel, RecordingEgress, StreamEvent};

fn main() {
    let symbol = std::env::var("SYMBOL").unwrap_or_else(|_| "BTCUSDT".into());
    let window_frames = std::env::var("WINDOW_FRAMES")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(1_usize);

    let mut config = EnvConfig::new(symbol.clone());
    config.window_frames = window_frames;
    if let Ok(qty) = std::env::var("DEFAULT_QTY") {
        config.default_qty = qty;
    }

    let mut env = Env::new(config, RecordingEgress::default());
    let policy = load_policy(window_frames);
    let messages = synthetic_depth_stream(&symbol);

    let steps = run_offline_policy_harness(&mut env, &policy, messages)
        .expect("synthetic stream and recording egress should not fail");

    println!("steps: {}", steps.len());
    for (idx, message) in env.egress().dispatched.iter().enumerate() {
        println!("dispatch[{idx}]: {message:?}");
    }
}

fn load_policy(window_frames: usize) -> CheckpointOrHoldPolicy {
    #[cfg(any(feature = "ort", feature = "torch"))]
    let obs_dim = trolly_gym::sim::microstructure_obs_dim(window_frames);
    #[cfg(not(any(feature = "ort", feature = "torch")))]
    let _ = window_frames;

    #[cfg(feature = "ort")]
    if let Some(path) = std::env::var_os("ONNX_MODEL_PATH") {
        return match CheckpointOrHoldPolicy::from_onnx_model(path, obs_dim) {
            Ok(policy) => policy,
            Err(err) => {
                eprintln!("ONNX model load failed; falling back to hold policy: {err}");
                CheckpointOrHoldPolicy::hold()
            }
        };
    }

    #[cfg(not(feature = "ort"))]
    if std::env::var_os("ONNX_MODEL_PATH").is_some() {
        eprintln!("ONNX_MODEL_PATH ignored because trolly-gym was built without --features ort");
    }

    #[cfg(feature = "torch")]
    {
        let Some(dir) = std::env::var_os("CHECKPOINT_DIR") else {
            return CheckpointOrHoldPolicy::hold();
        };

        return match CheckpointOrHoldPolicy::from_latest_checkpoint_dir(
            dir,
            obs_dim,
            Default::default(),
        ) {
            Ok(policy) => policy,
            Err(err) => {
                eprintln!("checkpoint load failed; falling back to hold policy: {err}");
                CheckpointOrHoldPolicy::hold()
            }
        };
    }

    #[cfg(not(feature = "torch"))]
    if std::env::var_os("CHECKPOINT_DIR").is_some() {
        eprintln!("CHECKPOINT_DIR ignored because trolly-gym was built without --features torch");
    }

    CheckpointOrHoldPolicy::hold()
}

fn synthetic_depth_stream(symbol: &str) -> Vec<trolly_stream::Message> {
    [
        ("100.00", "101.00", 1_u64),
        ("100.25", "101.25", 2_u64),
        ("100.50", "101.50", 3_u64),
    ]
    .into_iter()
    .map(|(bid, ask, update_id)| {
        envelope_message(&StreamEvent::Depth(DepthUpdate {
            symbol: symbol.into(),
            bids: vec![PriceLevel {
                price: bid.into(),
                qty: "1.0".into(),
            }],
            asks: vec![PriceLevel {
                price: ask.into(),
                qty: "1.0".into(),
            }],
            update_id: Some(update_id),
        }))
    })
    .collect()
}
