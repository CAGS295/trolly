//! Content hashes stored beside every updated checkpoint.
//!
//! Join node: every save advertises [`ModelFingerprint`] in
//! `latest.fingerprint.json` (and the completed-models manifest when a model
//! completes). Fulfillment is [`write_sidecar_for_checkpoint`] on that same
//! save path — do not hash in one place and serialize a different bag elsewhere.

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Sidecar written next to `latest.safetensors` on every save.
pub const FINGERPRINT_SIDECAR: &str = "latest.fingerprint.json";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelFingerprint {
    pub weights_sha256: String,
    pub data_window_sha256: String,
    pub config_sha256: String,
    pub checkpoint: String,
}

pub fn hash_bytes(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hex_encode(&hasher.finalize())
}

pub fn hash_file(path: impl AsRef<Path>) -> std::io::Result<String> {
    let bytes = fs::read(path)?;
    Ok(hash_bytes(&bytes))
}

pub fn hash_text(text: &str) -> String {
    hash_bytes(text.as_bytes())
}

/// Hash the checkpoint file and write the sidecar beside it.
pub fn write_sidecar_for_checkpoint(
    checkpoint_path: impl AsRef<Path>,
    data_window: &str,
    config: &str,
) -> std::io::Result<ModelFingerprint> {
    let checkpoint_path = checkpoint_path.as_ref();
    let weights_sha256 = hash_file(checkpoint_path)?;
    let fingerprint = ModelFingerprint {
        weights_sha256,
        data_window_sha256: hash_text(data_window),
        config_sha256: hash_text(config),
        checkpoint: checkpoint_path.display().to_string(),
    };
    let sidecar = sidecar_for_checkpoint(checkpoint_path);
    let text = serde_json::to_string_pretty(&fingerprint)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    fs::write(&sidecar, text)?;
    Ok(fingerprint)
}

pub fn load_sidecar(checkpoint_dir: impl AsRef<Path>) -> Option<ModelFingerprint> {
    let path = checkpoint_dir.as_ref().join(FINGERPRINT_SIDECAR);
    let text = fs::read_to_string(path).ok()?;
    serde_json::from_str(&text).ok()
}

pub fn sidecar_path(checkpoint_dir: impl AsRef<Path>) -> PathBuf {
    checkpoint_dir.as_ref().join(FINGERPRINT_SIDECAR)
}

/// `{stem}.fingerprint.json` next to the weights file (`latest.safetensors` →
/// `latest.fingerprint.json`).
pub fn sidecar_for_checkpoint(checkpoint_path: impl AsRef<Path>) -> PathBuf {
    let path = checkpoint_path.as_ref();
    let dir = path.parent().unwrap_or(Path::new("."));
    match path.file_stem().and_then(|s| s.to_str()) {
        Some(stem) => dir.join(format!("{stem}.fingerprint.json")),
        None => dir.join(FINGERPRINT_SIDECAR),
    }
}

fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push(HEX[(b >> 4) as usize] as char);
        out.push(HEX[(b & 0xf) as usize] as char);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sidecar_hashes_file_and_window() {
        let dir = std::env::temp_dir().join(format!("trolly-fp-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let ckpt = dir.join("latest.safetensors");
        fs::write(&ckpt, b"fake-weights").unwrap();
        let fp = write_sidecar_for_checkpoint(&ckpt, "session-a", "mlp hidden=20").unwrap();
        assert_eq!(fp.weights_sha256, hash_bytes(b"fake-weights"));
        assert_eq!(fp.data_window_sha256, hash_text("session-a"));
        assert_eq!(fp.config_sha256, hash_text("mlp hidden=20"));
        let loaded = load_sidecar(&dir).unwrap();
        assert_eq!(loaded, fp);
        let _ = fs::remove_dir_all(&dir);
    }
}
