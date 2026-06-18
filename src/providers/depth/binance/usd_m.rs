//! Binance USDM (USD-M futures) depth endpoints.
//!
//! ## RPI overlay streams (optional)
//!
//! Subscribe with an `RPI:` symbol prefix (CLI / `--sources`: `binance-usd-m:RPI:BTCUSDT`) to
//! open the `@rpiDepth@500ms` combined stream instead of `@depth`. The provider sends
//! `SET_PROPERTY combined=true` before `SUBSCRIBE` when any symbol in the batch uses RPI.
//!
//! REST snapshots always use the bare symbol (`BTCUSDT`); RPI is WebSocket-only. Parsed
//! `@rpiDepth` envelopes get [`crate::providers::RPI_PREFIX`] prepended to the update symbol so
//! multiplex routing keeps `@depth` and `@rpiDepth` on separate handlers
//! (`BTCUSDT` vs `RPI:BTCUSDT`). Global merge keys follow the subscription symbol, so RPI does
//! not fold into the canonical `BTCUSDT` merged book unless both are subscribed under the same
//! symbol name (avoid that for production merge).

use super::ApiURL;

use crate::providers::{ApiURL, Endpoints};

pub const RPI_PREFIX: &str = "RPI:";

/// Strip optional [`RPI_PREFIX`] from a subscription symbol; returns `(bare_symbol, is_rpi)`.
pub fn strip_rpi(symbol: &str) -> (&str, bool) {
    match symbol.strip_prefix(RPI_PREFIX) {
        Some(raw) => (raw, true),
        None => (symbol, false),
    }
}

#[derive(Clone)]
pub struct BinanceUsdM;

impl ApiURL for BinanceUsdM {
    const STREAM: &'static str = "wss://fstream.binance.com";
    const REST: &'static str = "https://fapi.binance.com/fapi/v1";
}

impl trolly_stream::VenueEndpoints for BinanceUsdM {
    fn websocket_url(&self) -> String {
        format!("{}/stream", Self::STREAM)
    }

    fn rest_api_url(&self, symbol: impl AsRef<str>) -> String {
        let (raw, _) = strip_rpi(symbol.as_ref());
        format!(
            "{}/depth?symbol={}&limit=1000",
            Self::REST,
            raw.to_uppercase()
        )
    }

    fn ws_subscriptions(&self, symbols: impl Iterator<Item = impl AsRef<str>>) -> Vec<String> {
        let mut has_rpi = false;
        let params: Vec<_> = symbols
            .map(|s| {
                let (raw, rpi) = strip_rpi(s.as_ref());
                has_rpi |= rpi;
                if rpi {
                    format!("{}@rpiDepth@500ms", raw.to_lowercase())
                } else {
                    format!("{}@depth", raw.to_lowercase())
                }
            })
            .collect();

        let subscribe = format!(
            r#"{{"method": "SUBSCRIBE", "params": {:?}, "id": 1}}"#,
            params
        );

        if has_rpi {
            vec![
                r#"{"method":"SET_PROPERTY","params":["combined",true],"id":0}"#.into(),
                subscribe,
            ]
        } else {
            vec![subscribe]
        }
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use trolly_stream::VenueEndpoints;

    #[test]
    fn subscription_standard_only_skips_set_property() {
        let symbols = ["btcusdt", "ethusdt"];
        let msgs = BinanceUsdM.ws_subscriptions(symbols.iter());
        assert_eq!(msgs.len(), 1);
        assert_eq!(
            msgs[0],
            r#"{"method": "SUBSCRIBE", "params": ["btcusdt@depth", "ethusdt@depth"], "id": 1}"#
        );
    }

    #[test]
    fn subscription_rpi_sends_set_property_first() {
        let symbols = ["RPI:BTCUSDT"];
        let msgs = BinanceUsdM.ws_subscriptions(symbols.iter());
        assert_eq!(msgs.len(), 2);
        assert_eq!(
            msgs[0],
            r#"{"method":"SET_PROPERTY","params":["combined",true],"id":0}"#
        );
        assert_eq!(
            msgs[1],
            r#"{"method": "SUBSCRIBE", "params": ["btcusdt@rpiDepth@500ms"], "id": 1}"#
        );
    }

    #[test]
    fn subscription_mixed_sends_set_property_first() {
        let symbols = ["BTCUSDT", "RPI:BTCUSDT", "ETHUSDT"];
        let msgs = BinanceUsdM.ws_subscriptions(symbols.iter());
        assert_eq!(msgs.len(), 2);
        assert_eq!(
            msgs[0],
            r#"{"method":"SET_PROPERTY","params":["combined",true],"id":0}"#
        );
        assert_eq!(
            msgs[1],
            r#"{"method": "SUBSCRIBE", "params": ["btcusdt@depth", "btcusdt@rpiDepth@500ms", "ethusdt@depth"], "id": 1}"#
        );
    }

    #[test]
    fn websocket_url() {
        let url: String = BinanceUsdM.websocket_url();
        assert_eq!(url, "wss://fstream.binance.com/stream");
    }

    #[test]
    fn rest_api_url() {
        let url = BinanceUsdM.rest_api_url("BTCUSDT");
        assert_eq!(
            url,
            "https://fapi.binance.com/fapi/v1/depth?symbol=BTCUSDT&limit=1000"
        );
    }

    #[test]
    fn rest_api_url_strips_rpi_prefix() {
        let url = BinanceUsdM.rest_api_url("RPI:BTCUSDT");
        assert_eq!(
            url,
            "https://fapi.binance.com/fapi/v1/depth?symbol=BTCUSDT&limit=1000"
        );
    }

    #[test]
    fn strip_rpi_with_prefix() {
        let (raw, is_rpi) = strip_rpi("RPI:BTCUSDT");
        assert_eq!(raw, "BTCUSDT");
        assert!(is_rpi);
    }

    #[test]
    fn strip_rpi_without_prefix() {
        let (raw, is_rpi) = strip_rpi("BTCUSDT");
        assert_eq!(raw, "BTCUSDT");
        assert!(!is_rpi);
    }
}
