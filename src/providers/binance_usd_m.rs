use super::{ApiURL, Endpoints};
use crate::monitor::Depth;

pub const RPI_PREFIX: &str = "RPI:";

#[derive(Clone)]
pub struct BinanceUsdM;

impl ApiURL for BinanceUsdM {
    const STREAM: &'static str = "wss://fstream.binance.com";
    const REST: &'static str = "https://fapi.binance.com/fapi/v1";
}

fn strip_rpi(symbol: &str) -> (&str, bool) {
    match symbol.strip_prefix(RPI_PREFIX) {
        Some(raw) => (raw, true),
        None => (symbol, false),
    }
}

impl Endpoints<Depth> for BinanceUsdM {
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
    use crate::monitor::Depth;

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
        let url: String = <BinanceUsdM as Endpoints<Depth>>::websocket_url(&BinanceUsdM);
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
