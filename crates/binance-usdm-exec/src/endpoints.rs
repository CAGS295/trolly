//! USDM user-data stream connection helpers and API credentials.
//!
//! ## Subscription setup
//!
//! Binance USD-M futures execution events are delivered on a **private** user-data
//! websocket keyed by `listenKey` (created via `POST /fapi/v1/listenKey`). Callers
//! supply an active key for stream bookkeeping; outbound order placement uses signed
//! REST `POST /fapi/v1/order` (see crate README).
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
    /// Override private user-data WebSocket host (defaults to [`Self::PRODUCTION_WS_BASE`]).
    ws_base: Option<String>,
}

impl UsdmUserDataStream {
    pub const PRODUCTION_WS_BASE: &'static str = "wss://fstream.binance.com";
    /// USDM demo private user-data host (`wss://fstream.binancefuture.com`).
    pub const DEMO_WS_BASE: &'static str = "wss://fstream.binancefuture.com";
    /// USDM demo REST base (`https://demo-fapi.binance.com`).
    pub const DEMO_REST_BASE: &'static str = "https://demo-fapi.binance.com";

    pub fn new(listen_key: impl Into<String>) -> Self {
        Self {
            listen_key: listen_key.into(),
            events_filter: None,
            ws_base: None,
        }
    }

    /// USDM demo private user-data stream (listen key from [`crate::client::UsdmListenKeyClient`]).
    pub fn demo(listen_key: impl Into<String>) -> Self {
        Self {
            listen_key: listen_key.into(),
            events_filter: None,
            ws_base: Some(Self::DEMO_WS_BASE.into()),
        }
    }

    pub fn with_events_filter(mut self, events: impl Into<String>) -> Self {
        self.events_filter = Some(events.into());
        self
    }

    pub fn with_ws_base(mut self, base: impl Into<String>) -> Self {
        self.ws_base = Some(base.into());
        self
    }

    /// Read-only REST depth snapshot URL on the USDM demo host.
    pub fn demo_rest_depth_url(symbol: impl AsRef<str>) -> String {
        format!(
            "{}/fapi/v1/depth?symbol={}&limit=1000",
            Self::DEMO_REST_BASE,
            symbol.as_ref().to_uppercase()
        )
    }

    fn effective_ws_base(&self) -> &str {
        self.ws_base
            .as_deref()
            .unwrap_or(Self::PRODUCTION_WS_BASE)
    }
}

impl VenueEndpoints for UsdmUserDataStream {
    fn websocket_url(&self) -> String {
        let base = self.effective_ws_base();
        match &self.events_filter {
            Some(events) => format!(
                "{base}/private/ws?listenKey={}&events={}",
                self.listen_key, events
            ),
            None => format!("{base}/private/ws/{}", self.listen_key),
        }
    }

    fn ws_subscriptions(&self, _symbols: impl Iterator<Item = impl AsRef<str>>) -> Vec<String> {
        // User-data streams are opened by listenKey URL; no SUBSCRIBE frame is required.
        Vec::new()
    }

    fn rest_api_url(&self, _symbol: impl AsRef<str>) -> String {
        // Order placement uses [`crate::client::UsdmOrderClient`] directly.
        String::new()
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
    fn ws_subscriptions_empty() {
        let ep = UsdmUserDataStream::new("k");
        assert!(ep.ws_subscriptions(["BTCUSDT"].iter()).is_empty());
    }

    #[test]
    fn demo_rest_depth_url_format() {
        let url = UsdmUserDataStream::demo_rest_depth_url("btcusdt");
        assert_eq!(
            url,
            "https://demo-fapi.binance.com/fapi/v1/depth?symbol=BTCUSDT&limit=1000"
        );
    }

    #[test]
    fn demo_websocket_url_uses_binancefuture_host() {
        let ep = UsdmUserDataStream::demo("abc123");
        assert_eq!(
            ep.websocket_url(),
            "wss://fstream.binancefuture.com/private/ws/abc123"
        );
    }
}
