//! Scaffold for a third depth venue. Wire real URLs and subscriptions when adding a new exchange.

use crate::monitor::Depth;

use crate::providers::Endpoints;

#[derive(Clone, Debug)]
pub struct Other;

impl Endpoints<Depth> for Other {
    fn websocket_url(&self) -> String {
        String::new()
    }

    fn rest_api_url(&self, symbol: impl AsRef<str>) -> String {
        format!("other://depth/{}", symbol.as_ref().to_uppercase())
    }

    fn ws_subscriptions(&self, _symbols: impl Iterator<Item = impl AsRef<str>>) -> Vec<String> {
        Vec::new()
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::providers::Endpoints;

    #[test]
    fn rest_api_url_scaffold() {
        let url = Other.rest_api_url("btcusdt");
        assert_eq!(url, "other://depth/BTCUSDT");
    }
}
