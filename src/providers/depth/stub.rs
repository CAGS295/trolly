//! Scaffold third venue: label is registered in [`registry`](super::registry) but
//! WebSocket / REST endpoints are not wired yet (see `src/providers/.todo`).

use crate::monitor::Depth;

use super::super::{ApiURL, Endpoints};

/// Placeholder depth provider for extension scaffolding.
#[derive(Clone)]
pub struct Stub;

impl ApiURL for Stub {
    const STREAM: &'static str = "wss://stub.example.invalid";
    const REST: &'static str = "https://stub.example.invalid/api/v1";
}

impl Endpoints<Depth> for Stub {
    fn websocket_url(&self) -> String {
        Self::STREAM.to_string()
    }

    fn rest_api_url(&self, symbol: impl AsRef<str>) -> String {
        format!(
            "{}/depth?symbol={}",
            Self::REST,
            symbol.as_ref().to_uppercase()
        )
    }

    fn ws_subscriptions(
        &self,
        symbols: impl Iterator<Item = impl AsRef<str>>,
    ) -> Vec<String> {
        let params: Vec<_> = symbols
            .map(|s| format!("{}@depth", s.as_ref().to_lowercase()))
            .collect();
        vec![format!(
            r#"{{"method": "SUBSCRIBE", "params": {:?}, "id": 1}}"#,
            params
        )]
    }
}
