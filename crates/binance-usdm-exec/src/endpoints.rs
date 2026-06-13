//! USDM user-data stream connection helpers.
//!
//! ## Subscription setup
//!
//! Binance USD-M futures execution events are delivered on a **private** user-data
//! websocket keyed by `listenKey` (created via `POST /fapi/v1/listenKey`). This crate
//! does not call REST trading or listen-key endpoints; callers supply an active key.
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
//!
//! ## Demo / testnet hosts
//!
//! - REST: `https://demo-fapi.binance.com` ([USDM demo](https://developers.binance.com/docs/derivatives/))
//! - Private user-data WS: `wss://fstream.binancefuture.com/private/ws/<listenKey>`
//! - Market streams: `wss://fstream.binancefuture.com`

use trolly_stream::VenueEndpoints;

/// Production USDM REST host (`/fapi/v1/...`).
pub const USDM_REST_BASE_URL: &str = "https://fapi.binance.com";

/// USDM **demo** hosts.
pub const USDM_DEMO_REST_BASE_URL: &str = "https://demo-fapi.binance.com";
pub const USDM_DEMO_WS_PRIVATE_BASE: &str = "wss://fstream.binancefuture.com/private/ws";
pub const USDM_DEMO_MARKET_STREAM_URL: &str = "wss://fstream.binancefuture.com";

/// Production private user-data websocket base (`.../private/ws`).
pub const USDM_PROD_WS_PRIVATE_BASE: &str = "wss://fstream.binance.com/private/ws";

/// REST depth snapshot URL for USDM (production or demo base).
pub fn usdm_depth_rest_url(base: &str, symbol: impl AsRef<str>, limit: u32) -> String {
    format!(
        "{}/fapi/v1/depth?symbol={}&limit={}",
        base.trim_end_matches('/'),
        symbol.as_ref().to_uppercase(),
        limit
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
    ws_private_base: &'static str,
}

impl UsdmUserDataStream {
    pub fn new(listen_key: impl Into<String>) -> Self {
        Self {
            listen_key: listen_key.into(),
            events_filter: None,
            ws_private_base: USDM_PROD_WS_PRIVATE_BASE,
        }
    }

    /// Demo/testnet private user-data stream (`wss://fstream.binancefuture.com/private/ws/...`).
    pub fn demo(listen_key: impl Into<String>) -> Self {
        Self {
            listen_key: listen_key.into(),
            events_filter: None,
            ws_private_base: USDM_DEMO_WS_PRIVATE_BASE,
        }
    }

    pub fn with_events_filter(mut self, events: impl Into<String>) -> Self {
        self.events_filter = Some(events.into());
        self
    }

    fn private_ws_url(&self) -> String {
        match &self.events_filter {
            Some(events) => format!(
                "{}?listenKey={}&events={}",
                self.ws_private_base, self.listen_key, events
            ),
            None => format!("{}/{}", self.ws_private_base, self.listen_key),
        }
    }
}

impl VenueEndpoints for UsdmUserDataStream {
    fn websocket_url(&self) -> String {
        self.private_ws_url()
    }

    fn ws_subscriptions(&self, _symbols: impl Iterator<Item = impl AsRef<str>>) -> Vec<String> {
        // User-data streams are opened by listenKey URL; no SUBSCRIBE frame is required.
        Vec::new()
    }

    fn rest_api_url(&self, symbol: impl AsRef<str>) -> String {
        let base = if self.ws_private_base == USDM_DEMO_WS_PRIVATE_BASE {
            USDM_DEMO_REST_BASE_URL
        } else {
            USDM_REST_BASE_URL
        };
        usdm_depth_rest_url(base, symbol, 100)
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
    fn demo_websocket_url_listen_key_path() {
        let ep = UsdmUserDataStream::demo("abc123");
        assert_eq!(
            ep.websocket_url(),
            "wss://fstream.binancefuture.com/private/ws/abc123"
        );
        assert!(ep
            .rest_api_url("btcusdt")
            .starts_with("https://demo-fapi.binance.com/fapi/v1/depth"));
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
    fn demo_websocket_url_with_events_filter() {
        let ep = UsdmUserDataStream::demo("abc123")
            .with_events_filter("ORDER_TRADE_UPDATE/ACCOUNT_UPDATE");
        assert_eq!(
            ep.websocket_url(),
            "wss://fstream.binancefuture.com/private/ws?listenKey=abc123&events=ORDER_TRADE_UPDATE/ACCOUNT_UPDATE"
        );
    }

    #[test]
    fn ws_subscriptions_empty() {
        let ep = UsdmUserDataStream::new("k");
        assert!(ep.ws_subscriptions(["BTCUSDT"].iter()).is_empty());
    }
}
