use serde::Deserialize;

pub mod depth;
mod binance_usd_m;

pub use depth::Binance;
pub use depth::Stub;
pub use binance_usd_m::{BinanceUsdM, RPI_PREFIX};
pub use trolly_stream::Endpoints;

trait ApiURL {
    const STREAM: &'static str;
    const REST: &'static str;
}

#[derive(Deserialize, PartialEq, Debug)]
pub struct NullResponse {
    id: u64,
    pub result: Option<String>,
}

#[cfg(test)]
mod test {
    use super::NullResponse;

    #[test]
    fn deserialize_empty_response() {
        let body = r#"{"result":null,"id":1}"#;
        let expected: NullResponse = serde_json::from_str(body).unwrap();
        assert_eq!(
            NullResponse {
                id: 1,
                result: None
            },
            expected
        );
    }
}
