//! USDM user-data stream connection helpers and REST endpoint constants.
//!
//! ## Subscription setup
//!
//! Binance USD-M futures execution events are delivered on a **private** user-data
//! websocket keyed by `listenKey` (created via `POST /fapi/v1/listenKey`). This crate
//! does not manage listen-key lifecycle automatically; callers must create and keep
//! the key alive (see [README](../README.md)).
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

/// API credentials for signed REST order placement and listen-key helpers.
#[derive(Clone, Debug)]
pub struct ApiCredentials {
    pub api_key: String,
    pub secret_key: String,
}

/// Production USDM REST base URL.
pub const DEFAULT_REST_API_URL: &str = "https://fapi.binance.com";

/// Binance **demo** USDM REST base — [USDM general info](https://developers.binance.com/docs/derivatives/usds-margined-futures/general-info).
pub const DEMO_REST_API_URL: &str = "https://demo-fapi.binance.com";

/// Production private user-data WebSocket base (`…/private/ws/<listenKey>`).
pub const DEFAULT_WS_PRIVATE_BASE: &str = "wss://fstream.binance.com/private/ws";

/// Binance **demo** USDM market / private stream host (demo-fapi REST pairs with this host).
pub const DEMO_WS_PRIVATE_BASE: &str = "wss://fstream.binancefuture.com/private/ws";

/// Demo REST depth snapshot URL (`GET /fapi/v1/depth`).
pub fn demo_rest_depth_url(symbol: impl AsRef<str>) -> String {
    format!(
        "{}/fapi/v1/depth?symbol={}&limit=1000",
        DEMO_REST_API_URL,
        symbol.as_ref().to_uppercase()
    )
}

/// Listen-key REST paths (caller owns create/keepalive HTTP).
pub const LISTEN_KEY_CREATE_PATH: &str = "/fapi/v1/listenKey";
pub const LISTEN_KEY_KEEPALIVE_PATH: &str = "/fapi/v1/listenKey";

/// Private USDM user-data stream endpoint for a single `listenKey`.
#[derive(Clone, Debug)]
pub struct UsdmUserDataStream {
    pub listen_key: String,
    /// When set, appended as `events=` query filter (slash-separated event names).
    pub events_filter: Option<String>,
    private_ws_base: Option<String>,
}

impl UsdmUserDataStream {
    pub fn new(listen_key: impl Into<String>) -> Self {
        Self {
            listen_key: listen_key.into(),
            events_filter: None,
            private_ws_base: None,
        }
    }

    /// User-data stream on the Binance USDM **demo** private WebSocket host.
    pub fn demo(listen_key: impl Into<String>) -> Self {
        Self {
            listen_key: listen_key.into(),
            events_filter: None,
            private_ws_base: Some(DEMO_WS_PRIVATE_BASE.into()),
        }
    }

    pub fn with_private_ws_base(mut self, base: impl Into<String>) -> Self {
        self.private_ws_base = Some(base.into());
        self
    }

    fn private_ws_base(&self) -> &str {
        self.private_ws_base
            .as_deref()
            .unwrap_or(DEFAULT_WS_PRIVATE_BASE)
    }

    pub fn with_events_filter(mut self, events: impl Into<String>) -> Self {
        self.events_filter = Some(events.into());
        self
    }
}

impl VenueEndpoints for UsdmUserDataStream {
    fn websocket_url(&self) -> String {
        let base = self.private_ws_base();
        match &self.events_filter {
            Some(events) => format!("{base}?listenKey={}&events={}", self.listen_key, events),
            None => format!("{base}/{}", self.listen_key),
        }
    }

    fn ws_subscriptions(&self, _symbols: impl Iterator<Item = impl AsRef<str>>) -> Vec<String> {
        // User-data streams are opened by listenKey URL; no SUBSCRIBE frame is required.
        Vec::new()
    }

    fn rest_api_url(&self, _symbol: impl AsRef<str>) -> String {
        DEFAULT_REST_API_URL.into()
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
    fn websocket_url_with_events_filter() {
        let ep = UsdmUserDataStream::new("abc123")
            .with_events_filter("ORDER_TRADE_UPDATE/ACCOUNT_UPDATE");
        assert_eq!(
            ep.websocket_url(),
            "wss://fstream.binance.com/private/ws?listenKey=abc123&events=ORDER_TRADE_UPDATE/ACCOUNT_UPDATE"
        );
    }

    #[test]
    fn demo_websocket_url_listen_key_path() {
        let ep = UsdmUserDataStream::demo("abc123");
        assert_eq!(
            ep.websocket_url(),
            "wss://fstream.binancefuture.com/private/ws/abc123"
        );
    }

    #[test]
    fn demo_rest_depth_url_format() {
        let url = super::demo_rest_depth_url("btcusdt");
        assert_eq!(
            url,
            "https://demo-fapi.binance.com/fapi/v1/depth?symbol=BTCUSDT&limit=1000"
        );
    }

    #[test]
    fn ws_subscriptions_empty() {
        let ep = UsdmUserDataStream::new("k");
        assert!(ep.ws_subscriptions(["BTCUSDT"].iter()).is_empty());
    }
}
