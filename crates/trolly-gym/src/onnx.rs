//! ONNX Runtime-backed policy provider.
//!
//! This module is compiled only with `--features ort`. It keeps live inference
//! independent from the libtorch-backed training/checkpoint path.

use std::{
    path::{Path, PathBuf},
    sync::Mutex,
};

use crate::{
    action::Action,
    policy::{decode_gaussian_mean_output, GaussianMeanDecodeError, PolicyProvider},
};

/// Errors returned by [`OnnxPolicy`] loading or fallible inference.
#[derive(Debug, Clone)]
pub enum OnnxPolicyError {
    InvalidObservationDim(i64),
    MissingModel(PathBuf),
    Load { path: PathBuf, source: String },
    Inference(String),
    EmptyOutput,
    ShortOutput { len: usize },
    UnexpectedMeanOutput { len: usize },
    NonFiniteMean,
    SessionPoisoned,
}

impl OnnxPolicyError {
    /// Best-effort classifier for environments where the native ORT runtime is
    /// unavailable. Tests use this to skip runtime-dependent assertions cleanly.
    pub fn is_runtime_unavailable(&self) -> bool {
        let source = match self {
            Self::Load { source, .. } | Self::Inference(source) => source,
            _ => return false,
        };
        let lower = source.to_ascii_lowercase();
        lower.contains("shared library")
            || lower.contains("dynamic library")
            || lower.contains("onnx runtime")
            || lower.contains("onnxruntime")
            || lower.contains("ort dylib")
    }
}

impl std::fmt::Display for OnnxPolicyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidObservationDim(dim) => {
                write!(f, "invalid ONNX observation dimension {dim}")
            }
            Self::MissingModel(path) => write!(f, "ONNX model {} does not exist", path.display()),
            Self::Load { path, source } => {
                write!(f, "load ONNX model {}: {source}", path.display())
            }
            Self::Inference(source) => write!(f, "ONNX inference failed: {source}"),
            Self::EmptyOutput => write!(f, "ONNX model returned no outputs"),
            Self::ShortOutput { len } => write!(
                f,
                "ONNX model returned {len} logits; expected at least {}",
                Action::COUNT
            ),
            Self::UnexpectedMeanOutput { len } => write!(
                f,
                "ONNX Gaussian μ head returned {len} values; expected [1] or [1,1]"
            ),
            Self::NonFiniteMean => write!(f, "ONNX Gaussian μ is not finite"),
            Self::SessionPoisoned => write!(f, "ONNX session lock was poisoned"),
        }
    }
}

impl std::error::Error for OnnxPolicyError {}

/// Policy backed by a static ONNX actor model.
#[derive(Debug)]
pub struct OnnxPolicy {
    session: Mutex<ort::session::Session>,
    obs_dim: usize,
}

impl OnnxPolicy {
    /// Load an ONNX model from disk.
    ///
    /// The model is expected to accept a single `f32` tensor shaped
    /// `[1, obs_dim]` and return logits whose first three entries correspond to
    /// `Hold`, `Buy`, and `Sell`.
    pub fn from_model(path: impl AsRef<Path>, obs_dim: i64) -> Result<Self, OnnxPolicyError> {
        let (session, obs_dim) = load_session(path, obs_dim)?;
        Ok(Self {
            session: Mutex::new(session),
            obs_dim,
        })
    }

    /// Run fallible inference and decode the action logits by argmax.
    pub fn try_act(&self, obs: &[f32]) -> Result<Action, OnnxPolicyError> {
        let padded = prepare_observation(obs, self.obs_dim);
        let input = ort::value::TensorRef::from_array_view(([1_usize, self.obs_dim], &padded[..]))
            .map_err(|err| OnnxPolicyError::Inference(err.to_string()))?;

        let mut session = self
            .session
            .lock()
            .map_err(|_| OnnxPolicyError::SessionPoisoned)?;
        let outputs = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            session.run(ort::inputs![input])
        })) {
            Ok(Ok(outputs)) => outputs,
            Ok(Err(err)) => return Err(OnnxPolicyError::Inference(err.to_string())),
            Err(payload) => {
                return Err(OnnxPolicyError::Inference(panic_payload_to_string(payload)))
            }
        };
        let output = outputs
            .values()
            .next()
            .ok_or(OnnxPolicyError::EmptyOutput)?;
        let (_, logits) = output
            .try_extract_tensor::<f32>()
            .map_err(|err| OnnxPolicyError::Inference(err.to_string()))?;

        decode_action_from_logits(logits)
    }
}

/// Policy backed by a static Gaussian μ head (`[1, V×5] → [1]` or `[1,1]`).
///
/// Mean action is quantized through [`Action::quantize_inventory`] (WP-035).
/// The 3-logit [`OnnxPolicy`] path is unchanged.
#[derive(Debug)]
pub struct OnnxGaussianMeanPolicy {
    session: Mutex<ort::session::Session>,
    obs_dim: usize,
    hold_deadzone: f32,
}

impl OnnxGaussianMeanPolicy {
    /// Load a static Gaussian μ ONNX graph from disk.
    pub fn from_model(
        path: impl AsRef<Path>,
        obs_dim: i64,
        hold_deadzone: f32,
    ) -> Result<Self, OnnxPolicyError> {
        let (session, obs_dim) = load_session(path, obs_dim)?;
        Ok(Self {
            session: Mutex::new(session),
            obs_dim,
            hold_deadzone,
        })
    }

    pub fn hold_deadzone(&self) -> f32 {
        self.hold_deadzone
    }

    /// Run fallible inference, decode μ, and quantize onto `{Hold,Buy,Sell}`.
    pub fn try_act(&self, obs: &[f32]) -> Result<Action, OnnxPolicyError> {
        let padded = prepare_observation(obs, self.obs_dim);
        let input = ort::value::TensorRef::from_array_view(([1_usize, self.obs_dim], &padded[..]))
            .map_err(|err| OnnxPolicyError::Inference(err.to_string()))?;

        let mut session = self
            .session
            .lock()
            .map_err(|_| OnnxPolicyError::SessionPoisoned)?;
        let outputs = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            session.run(ort::inputs![input])
        })) {
            Ok(Ok(outputs)) => outputs,
            Ok(Err(err)) => return Err(OnnxPolicyError::Inference(err.to_string())),
            Err(payload) => {
                return Err(OnnxPolicyError::Inference(panic_payload_to_string(payload)))
            }
        };
        let output = outputs
            .values()
            .next()
            .ok_or(OnnxPolicyError::EmptyOutput)?;
        let (_, mean) = output
            .try_extract_tensor::<f32>()
            .map_err(|err| OnnxPolicyError::Inference(err.to_string()))?;

        let target = decode_mean_output(mean)?;
        Ok(Action::quantize_inventory(target, self.hold_deadzone))
    }
}

impl PolicyProvider for OnnxGaussianMeanPolicy {
    fn act(&self, obs: &[f32]) -> Action {
        self.try_act(obs).unwrap_or(Action::Hold)
    }
}

fn load_session(
    path: impl AsRef<Path>,
    obs_dim: i64,
) -> Result<(ort::session::Session, usize), OnnxPolicyError> {
    let path = path.as_ref();
    let obs_dim = usize::try_from(obs_dim)
        .ok()
        .filter(|dim| *dim > 0)
        .ok_or(OnnxPolicyError::InvalidObservationDim(obs_dim))?;

    if !path.exists() {
        return Err(OnnxPolicyError::MissingModel(path.to_path_buf()));
    }

    let session = match std::panic::catch_unwind(|| {
        ort::session::Session::builder().and_then(|builder| builder.commit_from_file(path))
    }) {
        Ok(Ok(session)) => session,
        Ok(Err(err)) => {
            return Err(OnnxPolicyError::Load {
                path: path.to_path_buf(),
                source: err.to_string(),
            });
        }
        Err(payload) => {
            return Err(OnnxPolicyError::Load {
                path: path.to_path_buf(),
                source: panic_payload_to_string(payload),
            });
        }
    };

    Ok((session, obs_dim))
}

fn decode_mean_output(output: &[f32]) -> Result<f32, OnnxPolicyError> {
    decode_gaussian_mean_output(output).map_err(|err| match err {
        GaussianMeanDecodeError::Empty => OnnxPolicyError::EmptyOutput,
        GaussianMeanDecodeError::UnexpectedLen(len) => {
            OnnxPolicyError::UnexpectedMeanOutput { len }
        }
        GaussianMeanDecodeError::NonFinite => OnnxPolicyError::NonFiniteMean,
    })
}

fn panic_payload_to_string(payload: Box<dyn std::any::Any + Send>) -> String {
    match payload.downcast::<String>() {
        Ok(message) => *message,
        Err(payload) => match payload.downcast::<&'static str>() {
            Ok(message) => (*message).to_string(),
            Err(_) => "unknown panic while calling ONNX Runtime".to_string(),
        },
    }
}

impl PolicyProvider for OnnxPolicy {
    fn act(&self, obs: &[f32]) -> Action {
        self.try_act(obs).unwrap_or(Action::Hold)
    }
}

fn prepare_observation(obs: &[f32], obs_dim: usize) -> Vec<f32> {
    let mut padded = vec![0.0_f32; obs_dim];
    for (dst, src) in padded.iter_mut().zip(obs.iter().copied()) {
        *dst = src;
    }
    padded
}

fn decode_action_from_logits(logits: &[f32]) -> Result<Action, OnnxPolicyError> {
    let action_count = Action::COUNT as usize;
    if logits.len() < action_count {
        return Err(OnnxPolicyError::ShortOutput { len: logits.len() });
    }

    let mut best_index = 0_usize;
    let mut best_value = logits[0];
    for (idx, value) in logits
        .iter()
        .copied()
        .take(action_count)
        .enumerate()
        .skip(1)
    {
        if value > best_value || (best_value.is_nan() && !value.is_nan()) {
            best_index = idx;
            best_value = value;
        }
    }

    Ok(Action::from_index(best_index as i64))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_file(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "trolly_gym_onnx_{name}_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }

    #[test]
    fn decode_logits_by_argmax() {
        assert_eq!(
            decode_action_from_logits(&[0.9, 0.1, -0.5]).unwrap(),
            Action::Hold
        );
        assert_eq!(
            decode_action_from_logits(&[-1.0, 0.2, 0.1]).unwrap(),
            Action::Buy
        );
        assert_eq!(
            decode_action_from_logits(&[0.0, 0.1, 1.1]).unwrap(),
            Action::Sell
        );
    }

    #[test]
    fn decode_requires_three_logits() {
        assert!(matches!(
            decode_action_from_logits(&[0.0, 1.0]),
            Err(OnnxPolicyError::ShortOutput { len: 2 })
        ));
    }

    #[test]
    fn prepare_observation_pads_and_truncates() {
        assert_eq!(
            prepare_observation(&[1.0, 2.0], 4),
            vec![1.0, 2.0, 0.0, 0.0]
        );
        assert_eq!(prepare_observation(&[1.0, 2.0, 3.0], 2), vec![1.0, 2.0]);
    }

    #[test]
    fn missing_model_reports_without_loading_runtime() {
        let path = temp_file("missing_model.onnx");
        let err = OnnxPolicy::from_model(&path, 7).unwrap_err();
        assert!(matches!(err, OnnxPolicyError::MissingModel(_)));
    }

    #[test]
    fn missing_gaussian_model_reports_without_loading_runtime() {
        let path = temp_file("missing_gaussian.onnx");
        let err = OnnxGaussianMeanPolicy::from_model(&path, 40, 0.25).unwrap_err();
        assert!(matches!(err, OnnxPolicyError::MissingModel(_)));
    }

    #[test]
    fn decode_mean_output_accepts_scalar_and_row() {
        assert_eq!(decode_mean_output(&[0.8]).unwrap(), 0.8);
        assert_eq!(decode_mean_output(&[-0.4, 0.1]).unwrap(), -0.4);
        assert!(matches!(
            decode_mean_output(&[0.1, 0.2, 0.3]),
            Err(OnnxPolicyError::UnexpectedMeanOutput { len: 3 })
        ));
        assert!(matches!(
            decode_mean_output(&[]),
            Err(OnnxPolicyError::EmptyOutput)
        ));
    }

    #[test]
    fn decoded_mean_quantizes_to_dispatch_actions() {
        assert_eq!(
            Action::quantize_inventory(decode_mean_output(&[0.8]).unwrap(), 0.25),
            Action::Buy
        );
        assert_eq!(
            Action::quantize_inventory(decode_mean_output(&[0.05]).unwrap(), 0.25),
            Action::Hold
        );
        assert_eq!(
            Action::quantize_inventory(decode_mean_output(&[-0.8]).unwrap(), 0.25),
            Action::Sell
        );
    }

    #[test]
    fn invalid_model_reports_or_skips_when_runtime_unavailable() {
        let path = temp_file("invalid_model.onnx");
        std::fs::write(&path, b"not an onnx graph").unwrap();

        let err = OnnxPolicy::from_model(&path, 7).unwrap_err();
        let _ = std::fs::remove_file(&path);

        if err.is_runtime_unavailable() {
            eprintln!("skipping invalid-model assertion: {err}");
            return;
        }

        assert!(matches!(err, OnnxPolicyError::Load { .. }));
    }
}
