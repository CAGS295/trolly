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
}

impl UsdmUserDataStream {
    pub fn new(listen_key: impl Into<String>) -> Self {
        Self {
            listen_key: listen_key.into(),
            events_filter: None,
        }
    }

    pub fn with_events_filter(mut self, events: impl Into<String>) -> Self {
        self.events_filter = Some(events.into());
        self
    }
}

impl VenueEndpoints for UsdmUserDataStream {
    fn websocket_url(&self) -> String {
        match &self.events_filter {
            Some(events) => format!(
                "wss://fstream.binance.com/private/ws?listenKey={}&events={}",
                self.listen_key, events
            ),
            None => format!(
                "wss://fstream.binance.com/private/ws/{}",
                self.listen_key
            ),
        }
    }

    fn ws_subscriptions(&self, _symbols: impl Iterator<Item = impl AsRef<str>>) -> Vec<String> {
        // User-data streams are opened by listenKey URL; no SUBSCRIBE frame is required.
        Vec::new()
    }

    fn rest_api_url(&self, _symbol: impl AsRef<str>) -> String {
        // Listen-key REST is caller-owned; order placement uses [`crate::order`].
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
}
