//! USDM user-data stream websocket endpoints (listenKey-based, no REST trading).

use crate::events::UsdmExec;
use trolly_stream::Endpoints;

/// Binance USDM private user-data stream connection parameters.
///
/// Obtain a `listen_key` via `POST /fapi/v1/listenKey` (outside this crate).
/// Keep it alive with `PUT /fapi/v1/listenKey` every ~30 minutes.
/// Connect to [`Self::websocket_url`] — no additional subscribe frames are required.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UsdmUserDataEndpoints {
    pub listen_key: String,
}

impl UsdmUserDataEndpoints {
    pub fn new(listen_key: impl Into<String>) -> Self {
        Self {
            listen_key: listen_key.into(),
        }
    }

    /// Base URL for Binance USDM private user-data streams.
    pub const PRIVATE_STREAM: &'static str = "wss://fstream.binance.com/private/ws";
}

impl Endpoints<UsdmExec> for UsdmUserDataEndpoints {
    fn websocket_url(&self) -> String {
        format!("{}/{}", Self::PRIVATE_STREAM, self.listen_key)
    }

    fn ws_subscriptions(
        &self,
        _symbols: impl Iterator<Item = impl AsRef<str>>,
    ) -> Vec<String> {
        // User-data stream is authenticated by listenKey in the URL path.
        Vec::new()
    }

    fn rest_api_url(&self, _symbol: impl AsRef<str>) -> String {
        String::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn websocket_url_includes_listen_key() {
        let ep = UsdmUserDataEndpoints::new("abc123");
        assert_eq!(
            ep.websocket_url(),
            "wss://fstream.binance.com/private/ws/abc123"
        );
    }

    #[test]
    fn ws_subscriptions_empty() {
        let ep = UsdmUserDataEndpoints::new("abc123");
        assert!(ep.ws_subscriptions(["BTCUSDT"].iter()).is_empty());
    }
}
