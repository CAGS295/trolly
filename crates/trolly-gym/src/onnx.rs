//! ONNX Gaussian μ export (always) and Runtime-backed policy providers (`ort`).
//!
//! The writer emits a static `[1, V×5] → [1,1]` Gemm graph that
//! [`OnnxGaussianMeanPolicy`] / `ONNX_GAUSSIAN_MODEL_PATH` can load. Default
//! builds use a recorded-mean stand-in (zero weight, bias = μ) so CI never
//! needs libtorch. Weekday `gaussian_mlp` export is the Python script under
//! `scripts/export_gaussian_mu_onnx.py`.

use std::path::{Path, PathBuf};

/// Default flattened ladder (`V=8 × 5`) expected by weekday `gaussian_mlp`.
pub const DEFAULT_GAUSSIAN_MU_ONNX_OBS_DIM: i64 = 40;

/// Graph input name consumed by [`OnnxGaussianMeanPolicy`].
pub const GAUSSIAN_MU_ONNX_INPUT: &str = "observation";

/// Graph output name (`[1,1]` μ) consumed by [`OnnxGaussianMeanPolicy`].
pub const GAUSSIAN_MU_ONNX_OUTPUT: &str = "mean";

const RETIRED_UNIT_LOT_MICROSTRUCTURE: &str = "_retired_unit_lot_microstructure";
const ONNX_IR_VERSION: i64 = 8;
const ONNX_OPSET: i64 = 17;
const ONNX_FLOAT: i32 = 1;
const PRODUCER: &str = "trolly-gym";

/// Errors from the offline Gaussian μ ONNX writer / inspector.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OnnxExportError {
    InvalidObservationDim(i64),
    NonFiniteMean,
    RetiredUnitLot(PathBuf),
    Io(String),
    InvalidGraph(String),
}

impl std::fmt::Display for OnnxExportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidObservationDim(dim) => {
                write!(f, "invalid ONNX observation dimension {dim}")
            }
            Self::NonFiniteMean => write!(f, "recorded Gaussian μ is not finite"),
            Self::RetiredUnitLot(path) => {
                write!(f, "refusing retired unit-lot checkpoint {}", path.display())
            }
            Self::Io(source) => write!(f, "write ONNX Gaussian μ graph: {source}"),
            Self::InvalidGraph(source) => write!(f, "inspect ONNX Gaussian μ graph: {source}"),
        }
    }
}

impl std::error::Error for OnnxExportError {}

/// Metadata recovered from a graph written by [`write_recorded_mean_mu_onnx`].
#[derive(Debug, Clone, PartialEq)]
pub struct GaussianMuOnnxInfo {
    pub obs_dim: i64,
    pub mean_bias: f32,
    pub input_name: String,
    pub output_name: String,
    pub op_type: String,
    pub producer: String,
}

/// `true` when `path` sits under the retired 3-logit unit-lot fossil tree.
pub fn path_is_retired_unit_lot(path: impl AsRef<Path>) -> bool {
    path.as_ref().components().any(|c| {
        c.as_os_str()
            .to_string_lossy()
            .eq_ignore_ascii_case(RETIRED_UNIT_LOT_MICROSTRUCTURE)
    })
}

/// Write a recorded-mean stand-in `[1, obs_dim] → [1,1]` Gemm graph.
///
/// Weight is zeros; bias is `mean`. `OnnxGaussianMeanPolicy` treats the first
/// output value as μ and quantizes through WP-035. This is the offline path
/// used when a weekday `gaussian_mlp` checkpoint / libtorch is unavailable.
pub fn write_recorded_mean_mu_onnx(
    path: impl AsRef<Path>,
    obs_dim: i64,
    mean: f32,
) -> Result<GaussianMuOnnxInfo, OnnxExportError> {
    let path = path.as_ref();
    if path_is_retired_unit_lot(path) {
        return Err(OnnxExportError::RetiredUnitLot(path.to_path_buf()));
    }
    let obs_dim = usize::try_from(obs_dim)
        .ok()
        .filter(|dim| *dim > 0)
        .ok_or(OnnxExportError::InvalidObservationDim(obs_dim))?;
    if !mean.is_finite() {
        return Err(OnnxExportError::NonFiniteMean);
    }

    let weights = vec![0.0_f32; obs_dim];
    let bytes = encode_gemm_mu_onnx(obs_dim as i64, &weights, mean);
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent).map_err(|err| OnnxExportError::Io(err.to_string()))?;
        }
    }
    std::fs::write(path, &bytes).map_err(|err| OnnxExportError::Io(err.to_string()))?;
    inspect_gaussian_mu_onnx(&bytes)
}

/// Parse a graph produced by [`write_recorded_mean_mu_onnx`] (or the matching
/// Python stand-in writer). Used by default tests — no ORT / libtorch.
pub fn inspect_gaussian_mu_onnx(bytes: &[u8]) -> Result<GaussianMuOnnxInfo, OnnxExportError> {
    let model = ProtoMap::parse(bytes).map_err(OnnxExportError::InvalidGraph)?;
    let producer = model.string_field(2).unwrap_or_default().to_string();
    let graph = model
        .message_field(7)
        .ok_or_else(|| OnnxExportError::InvalidGraph("missing graph".into()))?;

    let mut input_name = String::new();
    let mut obs_dim = 0_i64;
    for input in graph.repeated_messages(11) {
        if let Some(name) = input.string_field(1) {
            if name == GAUSSIAN_MU_ONNX_INPUT || input_name.is_empty() {
                input_name = name.to_string();
                if let Some(dim) = tensor_last_dim(&input) {
                    obs_dim = dim;
                }
            }
        }
    }

    let output_name = graph
        .repeated_messages(12)
        .next()
        .and_then(|output| output.string_field(1).map(str::to_string))
        .unwrap_or_default();

    let op_type = graph
        .repeated_messages(1)
        .next()
        .and_then(|node| node.string_field(4).map(str::to_string))
        .unwrap_or_default();

    let mut mean_bias = f32::NAN;
    for initializer in graph.repeated_messages(5) {
        if initializer.string_field(1).is_none() {
            // TensorProto.name is field 8
        }
        if initializer.string_field(8) == Some("B") {
            mean_bias = initializer_f32(&initializer).ok_or_else(|| {
                OnnxExportError::InvalidGraph("bias initializer is not a single f32".into())
            })?;
        }
    }

    if input_name.is_empty()
        || output_name.is_empty()
        || op_type.is_empty()
        || !mean_bias.is_finite()
    {
        return Err(OnnxExportError::InvalidGraph(
            "graph is missing observation/mean/Gemm/B".into(),
        ));
    }

    Ok(GaussianMuOnnxInfo {
        obs_dim,
        mean_bias,
        input_name,
        output_name,
        op_type,
        producer,
    })
}

fn tensor_last_dim(value_info: &ProtoMap<'_>) -> Option<i64> {
    let ty = value_info.message_field(2)?;
    let tensor = ty.message_field(1)?;
    let shape = tensor.message_field(2)?;
    shape
        .repeated_messages(1)
        .last()
        .and_then(|dim| dim.varint_field(1))
        .map(|v| v as i64)
}

fn initializer_f32(tensor: &ProtoMap<'_>) -> Option<f32> {
    if let Some(raw) = tensor.bytes_field(9) {
        if raw.len() >= 4 {
            return Some(f32::from_le_bytes(raw[..4].try_into().ok()?));
        }
    }
    None
}

fn encode_gemm_mu_onnx(obs_dim: i64, weights: &[f32], bias: f32) -> Vec<u8> {
    let w = encode_tensor("W", &[obs_dim, 1], weights);
    let b = encode_tensor("B", &[1], &[bias]);
    let input = encode_value_info(GAUSSIAN_MU_ONNX_INPUT, &[1, obs_dim]);
    let output = encode_value_info(GAUSSIAN_MU_ONNX_OUTPUT, &[1, 1]);
    let node = encode_node(
        "gemm_mu",
        "Gemm",
        &[GAUSSIAN_MU_ONNX_INPUT, "W", "B"],
        &[GAUSSIAN_MU_ONNX_OUTPUT],
    );

    let mut graph = Vec::new();
    put_len(&mut graph, 1, &node);
    put_string(&mut graph, 2, "gaussian_mu");
    put_len(&mut graph, 5, &w);
    put_len(&mut graph, 5, &b);
    put_len(&mut graph, 11, &input);
    put_len(&mut graph, 12, &output);

    let mut opset = Vec::new();
    put_string(&mut opset, 1, "");
    put_varint(&mut opset, 2, ONNX_OPSET as u64);

    let mut model = Vec::new();
    put_varint(&mut model, 1, ONNX_IR_VERSION as u64);
    put_string(&mut model, 2, PRODUCER);
    put_len(&mut model, 7, &graph);
    put_len(&mut model, 8, &opset);
    model
}

fn encode_tensor(name: &str, dims: &[i64], values: &[f32]) -> Vec<u8> {
    let mut buf = Vec::new();
    for dim in dims {
        put_varint(&mut buf, 1, *dim as u64);
    }
    put_varint(&mut buf, 2, ONNX_FLOAT as u64);
    put_string(&mut buf, 8, name);
    let mut raw = Vec::with_capacity(values.len() * 4);
    for value in values {
        raw.extend_from_slice(&value.to_le_bytes());
    }
    put_bytes(&mut buf, 9, &raw);
    buf
}

fn encode_value_info(name: &str, dims: &[i64]) -> Vec<u8> {
    let mut shape = Vec::new();
    for dim in dims {
        let mut dimension = Vec::new();
        put_varint(&mut dimension, 1, *dim as u64);
        put_len(&mut shape, 1, &dimension);
    }
    let mut tensor = Vec::new();
    put_varint(&mut tensor, 1, ONNX_FLOAT as u64);
    put_len(&mut tensor, 2, &shape);
    let mut ty = Vec::new();
    put_len(&mut ty, 1, &tensor);
    let mut info = Vec::new();
    put_string(&mut info, 1, name);
    put_len(&mut info, 2, &ty);
    info
}

fn encode_node(name: &str, op_type: &str, inputs: &[&str], outputs: &[&str]) -> Vec<u8> {
    let mut buf = Vec::new();
    for input in inputs {
        put_string(&mut buf, 1, input);
    }
    for output in outputs {
        put_string(&mut buf, 2, output);
    }
    put_string(&mut buf, 3, name);
    put_string(&mut buf, 4, op_type);
    buf
}

const WIRE_VARINT: u32 = 0;
const WIRE_LEN: u32 = 2;

fn put_tag(buf: &mut Vec<u8>, field: u32, wire: u32) {
    put_raw_varint(buf, u64::from((field << 3) | wire));
}

fn put_raw_varint(buf: &mut Vec<u8>, mut value: u64) {
    loop {
        let mut byte = (value & 0x7f) as u8;
        value >>= 7;
        if value != 0 {
            byte |= 0x80;
        }
        buf.push(byte);
        if value == 0 {
            break;
        }
    }
}

fn put_varint(buf: &mut Vec<u8>, field: u32, value: u64) {
    put_tag(buf, field, WIRE_VARINT);
    put_raw_varint(buf, value);
}

fn put_bytes(buf: &mut Vec<u8>, field: u32, value: &[u8]) {
    put_tag(buf, field, WIRE_LEN);
    put_raw_varint(buf, value.len() as u64);
    buf.extend_from_slice(value);
}

fn put_string(buf: &mut Vec<u8>, field: u32, value: &str) {
    put_bytes(buf, field, value.as_bytes());
}

fn put_len(buf: &mut Vec<u8>, field: u32, value: &[u8]) {
    put_bytes(buf, field, value);
}

#[derive(Clone)]
struct ProtoMap<'a> {
    fields: Vec<(u32, &'a [u8], u64)>,
}

impl<'a> ProtoMap<'a> {
    fn parse(bytes: &'a [u8]) -> Result<Self, String> {
        let mut fields = Vec::new();
        let mut pos = 0_usize;
        while pos < bytes.len() {
            let (key, next) = read_varint(bytes, pos)?;
            pos = next;
            let field = (key >> 3) as u32;
            let wire = (key & 7) as u32;
            match wire {
                WIRE_VARINT => {
                    let (value, next) = read_varint(bytes, pos)?;
                    pos = next;
                    fields.push((field, &[][..], value));
                }
                WIRE_LEN => {
                    let (len, next) = read_varint(bytes, pos)?;
                    pos = next;
                    let end = pos
                        .checked_add(len as usize)
                        .filter(|end| *end <= bytes.len())
                        .ok_or_else(|| "truncated length-delimited field".to_string())?;
                    fields.push((field, &bytes[pos..end], 0));
                    pos = end;
                }
                1 => {
                    pos = pos
                        .checked_add(8)
                        .filter(|end| *end <= bytes.len())
                        .ok_or_else(|| "truncated 64-bit field".to_string())?;
                }
                5 => {
                    pos = pos
                        .checked_add(4)
                        .filter(|end| *end <= bytes.len())
                        .ok_or_else(|| "truncated 32-bit field".to_string())?;
                }
                other => return Err(format!("unsupported protobuf wire type {other}")),
            }
        }
        Ok(Self { fields })
    }

    fn string_field(&self, field: u32) -> Option<&'a str> {
        self.fields
            .iter()
            .find(|(id, _, _)| *id == field)
            .and_then(|(_, bytes, _)| std::str::from_utf8(bytes).ok())
    }

    fn bytes_field(&self, field: u32) -> Option<&'a [u8]> {
        self.fields
            .iter()
            .find(|(id, bytes, _)| *id == field && !bytes.is_empty())
            .map(|(_, bytes, _)| *bytes)
    }

    fn varint_field(&self, field: u32) -> Option<u64> {
        self.fields
            .iter()
            .find(|(id, bytes, _)| *id == field && bytes.is_empty())
            .map(|(_, _, value)| *value)
    }

    fn message_field(&self, field: u32) -> Option<ProtoMap<'a>> {
        self.bytes_field(field)
            .and_then(|bytes| ProtoMap::parse(bytes).ok())
    }

    fn repeated_messages(&self, field: u32) -> impl Iterator<Item = ProtoMap<'a>> + '_ {
        self.fields
            .iter()
            .filter(move |(id, bytes, _)| *id == field && !bytes.is_empty())
            .filter_map(|(_, bytes, _)| ProtoMap::parse(bytes).ok())
    }
}

fn read_varint(bytes: &[u8], mut pos: usize) -> Result<(u64, usize), String> {
    let mut value = 0_u64;
    let mut shift = 0_u32;
    loop {
        let byte = *bytes
            .get(pos)
            .ok_or_else(|| "truncated varint".to_string())?;
        pos += 1;
        value |= u64::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            return Ok((value, pos));
        }
        shift += 7;
        if shift > 63 {
            return Err("varint overflow".into());
        }
    }
}

#[cfg(feature = "ort")]
mod runtime {
    use super::*;
    use std::sync::Mutex;

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
                Self::MissingModel(path) => {
                    write!(f, "ONNX model {} does not exist", path.display())
                }
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
            let input =
                ort::value::TensorRef::from_array_view(([1_usize, self.obs_dim], &padded[..]))
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
            let input =
                ort::value::TensorRef::from_array_view(([1_usize, self.obs_dim], &padded[..]))
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

        #[test]
        fn recorded_mean_export_is_loadable_or_skips_without_runtime() {
            let path = temp_file("exported_mu.onnx");
            let info = crate::onnx::write_recorded_mean_mu_onnx(&path, 40, 0.8).unwrap();
            assert_eq!(info.obs_dim, 40);
            assert!((info.mean_bias - 0.8).abs() < f32::EPSILON);

            let loaded = OnnxGaussianMeanPolicy::from_model(&path, 40, 0.25);
            let _ = std::fs::remove_file(&path);
            match loaded {
                Ok(policy) => {
                    assert_eq!(policy.try_act(&[0.0; 40]).unwrap(), Action::Buy);
                }
                Err(err) if err.is_runtime_unavailable() => {
                    eprintln!("skipping exported-mu runtime assertion: {err}");
                }
                Err(err) => panic!("exported μ graph should load: {err}"),
            }
        }
    }
}

#[cfg(feature = "ort")]
pub use runtime::{OnnxGaussianMeanPolicy, OnnxPolicy, OnnxPolicyError};

#[cfg(test)]
mod export_tests {
    use super::*;
    use crate::action::Action;
    use crate::policy::decode_gaussian_mean_output;

    fn temp_file(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "trolly_gym_onnx_export_{name}_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }

    #[test]
    fn recorded_mean_standin_writes_static_ladder_mu_graph() {
        let path = temp_file("mu.onnx");
        let info =
            write_recorded_mean_mu_onnx(&path, DEFAULT_GAUSSIAN_MU_ONNX_OBS_DIM, 0.8).unwrap();
        let bytes = std::fs::read(&path).unwrap();
        let _ = std::fs::remove_file(&path);

        assert_eq!(info.obs_dim, 40);
        assert_eq!(info.input_name, GAUSSIAN_MU_ONNX_INPUT);
        assert_eq!(info.output_name, GAUSSIAN_MU_ONNX_OUTPUT);
        assert_eq!(info.op_type, "Gemm");
        assert_eq!(info.producer, PRODUCER);
        assert!((info.mean_bias - 0.8).abs() < f32::EPSILON);
        assert!(bytes.windows(b"Gemm".len()).any(|w| w == b"Gemm"));
        assert_eq!(
            Action::quantize_inventory(
                decode_gaussian_mean_output(&[info.mean_bias]).unwrap(),
                0.25
            ),
            Action::Buy
        );
    }

    #[test]
    fn recorded_mean_standin_hold_and_sell_quantize() {
        let hold = write_recorded_mean_mu_onnx(temp_file("hold.onnx"), 40, 0.05).unwrap();
        let sell = write_recorded_mean_mu_onnx(temp_file("sell.onnx"), 40, -0.8).unwrap();
        assert_eq!(
            Action::quantize_inventory(
                decode_gaussian_mean_output(&[hold.mean_bias]).unwrap(),
                0.25
            ),
            Action::Hold
        );
        assert_eq!(
            Action::quantize_inventory(
                decode_gaussian_mean_output(&[sell.mean_bias]).unwrap(),
                0.25
            ),
            Action::Sell
        );
    }

    #[test]
    fn export_refuses_retired_unit_lot_path() {
        let path = PathBuf::from("/tmp/_retired_unit_lot_microstructure/mu.onnx");
        let err = write_recorded_mean_mu_onnx(&path, 40, 0.8).unwrap_err();
        assert!(matches!(err, OnnxExportError::RetiredUnitLot(_)));
        assert!(path_is_retired_unit_lot(
            "checkpoints/gpu_train_orchestrator/_retired_unit_lot_microstructure"
        ));
    }

    #[test]
    fn export_rejects_invalid_obs_dim_and_nan_mean() {
        let path = temp_file("bad.onnx");
        assert!(matches!(
            write_recorded_mean_mu_onnx(&path, 0, 0.8),
            Err(OnnxExportError::InvalidObservationDim(0))
        ));
        assert!(matches!(
            write_recorded_mean_mu_onnx(&path, 40, f32::NAN),
            Err(OnnxExportError::NonFiniteMean)
        ));
    }
}
