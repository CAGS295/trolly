/// Venue connection endpoints for websocket streams and REST snapshots.
pub trait VenueEndpoints {
    fn websocket_url(&self) -> String;
    fn ws_subscriptions(&self, symbols: impl Iterator<Item = impl AsRef<str>>) -> Vec<String>;
    fn rest_api_url(&self, symbol: impl AsRef<str>) -> String;
}

/// WebSocket-only view of [`VenueEndpoints`] (used by multiplexor subscribe).
pub trait StreamEndpoints {
    fn websocket_url(&self) -> String;
    fn ws_subscriptions(&self, symbols: impl Iterator<Item = impl AsRef<str>>) -> Vec<String>;
}

impl<T: VenueEndpoints> StreamEndpoints for T {
    fn websocket_url(&self) -> String {
        VenueEndpoints::websocket_url(self)
    }

    fn ws_subscriptions(&self, symbols: impl Iterator<Item = impl AsRef<str>>) -> Vec<String> {
        VenueEndpoints::ws_subscriptions(self, symbols)
    }
}
