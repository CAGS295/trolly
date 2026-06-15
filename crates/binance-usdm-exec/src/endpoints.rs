//! USDM user-data stream connection helpers.
//!
//! ## Subscription setup
//!
//! Binance USD-M futures execution events are delivered on a **private** user-data
//! websocket keyed by `listenKey` (created via `POST /fapi/v1/listenKey`). Use
//! [`crate::listen_key::ListenKeyClient`] for listen-key lifecycle on demo or production REST.
//!
//! Recommended private URL (2026 endpoint layout):
//!
//! ```text
//! wss://fstream.binance.com/private/ws/<listenKey>
//! ```
//!
//! Optional event filtering via query string:
//!
//! ```text
//! wss://fstream.binance.com/private/ws?listenKey=<key>&events=ORDER_TRADE_UPDATE/ACCOUNT_UPDATE
//! ```
//!
//! Fan execution events into a shared [`trolly_stream::MonitorMultiplexor`] via
//! [`crate::ingress::ingest_user_data`] so USDM updates route alongside spot depth
//! and execution handlers on the same ingress API.

use trolly_stream::VenueEndpoints;

/// Binance USDM demo REST host.
pub const DEMO_REST_BASE_URL: &str = "https://demo-fapi.binance.com";
/// Binance USDM demo private user-data WebSocket base.
pub const DEMO_PRIVATE_WS_BASE: &str = "wss://fstream.binancefuture.com/private";
/// Binance USDM demo public market streams host.
pub const DEMO_STREAM_BASE: &str = "wss://fstream.binancefuture.com";

/// Demo REST depth snapshot URL for a symbol.
pub fn demo_depth_rest_url(symbol: impl AsRef<str>) -> String {
    format!(
        "{}/fapi/v1/depth?symbol={}&limit=1000",
        DEMO_REST_BASE_URL.trim_end_matches('/'),
        symbol.as_ref().to_uppercase()
    )
}

/// API credentials for signed REST order placement.
#[derive(Clone, Debug)]
pub struct ApiCredentials {
    pub api_key: String,
    pub secret_key: String,
}

/// Private USDM user-data stream endpoint for a single `listenKey`.
#[derive(Clone, Debug)]
pub struct UsdmUserDataStream {
    pub listen_key: String,
    /// When set, appended as `events=` query filter (slash-separated event names).
    pub events_filter: Option<String>,
    private_ws_base: String,
}

impl UsdmUserDataStream {
    pub const PRIVATE_WS_BASE: &'static str = "wss://fstream.binance.com/private";

    pub fn new(listen_key: impl Into<String>) -> Self {
        Self {
            listen_key: listen_key.into(),
            events_filter: None,
            private_ws_base: Self::PRIVATE_WS_BASE.into(),
        }
    }

    /// User-data stream on Binance USDM **demo** private WebSocket hosts.
    pub fn demo(listen_key: impl Into<String>) -> Self {
        Self {
            listen_key: listen_key.into(),
            events_filter: None,
            private_ws_base: DEMO_PRIVATE_WS_BASE.into(),
        }
    }

    pub fn with_private_ws_base(mut self, base: impl Into<String>) -> Self {
        self.private_ws_base = base.into();
        self
    }

    pub fn with_events_filter(mut self, events: impl Into<String>) -> Self {
        self.events_filter = Some(events.into());
        self
    }
}

impl VenueEndpoints for UsdmUserDataStream {
    fn websocket_url(&self) -> String {
        let base = self.private_ws_base.trim_end_matches('/');
        match &self.events_filter {
            Some(events) => format!(
                "{base}/ws?listenKey={}&events={}",
                self.listen_key, events
            ),
            None => format!("{base}/ws/{}", self.listen_key),
        }
    }

    fn ws_subscriptions(&self, _symbols: impl Iterator<Item = impl AsRef<str>>) -> Vec<String> {
        // User-data streams are opened by listenKey URL; no SUBSCRIBE frame is required.
        Vec::new()
    }

    fn rest_api_url(&self, _symbol: impl AsRef<str>) -> String {
        crate::order::DEFAULT_REST_BASE_URL.into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use trolly_stream::VenueEndpoints;

    #[test]
    fn websocket_url_listen_key_path() {
        let ep = UsdmUserDataStream::new("abc123");
        assert_eq!(
            ep.websocket_url(),
            "wss://fstream.binance.com/private/ws/abc123"
        );
    }

    #[test]
    fn demo_websocket_url_uses_demo_private_host() {
        let ep = UsdmUserDataStream::demo("abc123");
        assert_eq!(
            ep.websocket_url(),
            "wss://fstream.binancefuture.com/private/ws/abc123"
        );
    }

    #[test]
    fn demo_depth_rest_url_builds_expected_path() {
        assert_eq!(
            demo_depth_rest_url("ethusdt"),
            "https://demo-fapi.binance.com/fapi/v1/depth?symbol=ETHUSDT&limit=1000"
        );
    }

    #[test]
    fn websocket_url_with_events_filter() {
        let ep = UsdmUserDataStream::new("abc123")
            .with_events_filter("ORDER_TRADE_UPDATE/ACCOUNT_UPDATE");
        assert_eq!(
            ep.websocket_url(),
            "wss://fstream.binance.com/private/ws?listenKey=abc123&events=ORDER_TRADE_UPDATE/ACCOUNT_UPDATE"
        );
    }

    #[test]
    fn ws_subscriptions_empty() {
        let ep = UsdmUserDataStream::new("k");
        assert!(ep.ws_subscriptions(["BTCUSDT"].iter()).is_empty());
    }
}
