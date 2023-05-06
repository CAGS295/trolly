pub mod binance;

pub(crate) use binance::Binance;

pub(crate) trait Endpoints<Monitorable> {
    fn websocket_url(&self, symbol: impl AsRef<str>) -> String;
    fn rest_api_url(&self, symbol: impl AsRef<str>) -> String;
}

trait ApiURL {
    const STREAM: &'static str;
    const REST: &'static str;
}
