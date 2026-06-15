use std::collections::BTreeMap;
use std::time::{SystemTime, UNIX_EPOCH};

use hmac::{Hmac, Mac};
use sha2::Sha256;

type HmacSha256 = Hmac<Sha256>;

pub fn current_timestamp_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time before UNIX epoch")
        .as_millis() as u64
}

/// HMAC-SHA256 hex signature for Binance WebSocket API signed params.
pub fn sign_hmac_sha256_hex(secret_key: &str, payload: &str) -> String {
    let mut mac = HmacSha256::new_from_slice(secret_key.as_bytes()).expect("HMAC key length");
    mac.update(payload.as_bytes());
    hex::encode(mac.finalize().into_bytes())
}

pub fn signed_params_payload(params: &BTreeMap<String, String>) -> String {
    params
        .iter()
        .map(|(key, value)| format!("{key}={value}"))
        .collect::<Vec<_>>()
        .join("&")
}

pub fn append_signature(secret_key: &str, params: &mut BTreeMap<String, String>) {
    let payload = signed_params_payload(params);
    let signature = sign_hmac_sha256_hex(secret_key, &payload);
    params.insert("signature".into(), signature);
}

pub fn build_subscribe_signature_params(api_key: &str, secret_key: &str) -> BTreeMap<String, String> {
    let timestamp = current_timestamp_ms().to_string();
    let mut params = BTreeMap::new();
    params.insert("apiKey".into(), api_key.into());
    params.insert("timestamp".into(), timestamp);
    let payload = signed_params_payload(&params);
    let signature = sign_hmac_sha256_hex(secret_key, &payload);
    params.insert("signature".into(), signature);
    params
}
