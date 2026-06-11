/// Provider connection surface for websocket subscription and REST snapshots.
pub trait Endpoints<Monitorable> {
    fn websocket_url(&self) -> String;
    fn ws_subscriptions(&self, symbols: impl Iterator<Item = impl AsRef<str>>) -> Vec<String>;
    fn rest_api_url(&self, symbol: impl AsRef<str>) -> String;
}
