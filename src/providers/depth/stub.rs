//! Scaffold third venue for `--sources stub:SYMBOL` registration (WP-003).
//! Not wired to a live exchange; endpoints are placeholders for future venues.

#[derive(Clone)]
pub struct Stub;

impl trolly_stream::VenueEndpoints for Stub {
    fn websocket_url(&self) -> String {
        "wss://stub.example/ws".into()
    }

    fn rest_api_url(&self, symbol: impl AsRef<str>) -> String {
        format!(
            "https://stub.example/depth?symbol={}",
            symbol.as_ref().to_uppercase()
        )
    }

    fn ws_subscriptions(&self, symbols: impl Iterator<Item = impl AsRef<str>>) -> Vec<String> {
        let params: Vec<_> = symbols
            .map(|s| format!("{}@depth", s.as_ref().to_lowercase()))
            .collect();
        vec![format!(
            r#"{{"method": "SUBSCRIBE", "params": {:?}, "id": 1}}"#,
            params
        )]
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use trolly_stream::VenueEndpoints;

    #[test]
    fn websocket_url() {
        assert_eq!(Stub.websocket_url(), "wss://stub.example/ws");
    }
}
