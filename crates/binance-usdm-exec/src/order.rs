//! Signed REST order placement (`POST /fapi/v1/order`).
//!
//! Fills and rejects are reconciled via the existing user-data `ORDER_TRADE_UPDATE`
//! stream path — this module does not maintain a separate order state machine.

use std::collections::BTreeMap;

use serde::Deserialize;
use thiserror::Error;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

use crate::auth::{current_timestamp_ms, sign_hmac_sha256_hex, signed_params_payload};
use crate::endpoints::ApiCredentials;

/// REST API host for USDM order placement.
pub const REST_BASE_URL: &str = "https://fapi.binance.com";

/// Order side (`BUY` / `SELL`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OrderSide {
    Buy,
    Sell,
}

impl OrderSide {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Buy => "BUY",
            Self::Sell => "SELL",
        }
    }

    pub fn parse(value: &str) -> Result<Self, OrderBuilderError> {
        match value.to_ascii_uppercase().as_str() {
            "BUY" => Ok(Self::Buy),
            "SELL" => Ok(Self::Sell),
            other => Err(OrderBuilderError::InvalidSide(other.into())),
        }
    }
}

/// USDM order type (`MARKET` / `LIMIT`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OrderType {
    Market,
    Limit,
}

impl OrderType {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Market => "MARKET",
            Self::Limit => "LIMIT",
        }
    }
}

/// Time in force for limit orders.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimeInForce {
    Gtc,
    Ioc,
    Fok,
    Gtx,
}

impl TimeInForce {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Gtc => "GTC",
            Self::Ioc => "IOC",
            Self::Fok => "FOK",
            Self::Gtx => "GTX",
        }
    }

    pub fn parse(value: &str) -> Result<Self, OrderBuilderError> {
        match value.to_ascii_uppercase().as_str() {
            "GTC" => Ok(Self::Gtc),
            "IOC" => Ok(Self::Ioc),
            "FOK" => Ok(Self::Fok),
            "GTX" => Ok(Self::Gtx),
            other => Err(OrderBuilderError::InvalidTimeInForce(other.into())),
        }
    }
}

/// Position side for hedge mode (`LONG` / `SHORT` / `BOTH`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PositionSide {
    Long,
    Short,
    Both,
}

impl PositionSide {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Long => "LONG",
            Self::Short => "SHORT",
            Self::Both => "BOTH",
        }
    }

    pub fn parse(value: &str) -> Result<Self, OrderBuilderError> {
        match value.to_ascii_uppercase().as_str() {
            "LONG" => Ok(Self::Long),
            "SHORT" => Ok(Self::Short),
            "BOTH" => Ok(Self::Both),
            other => Err(OrderBuilderError::InvalidPositionSide(other.into())),
        }
    }
}

/// Request to place a USDM futures order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlaceOrderRequest {
    pub symbol: String,
    pub side: OrderSide,
    pub order_type: OrderType,
    pub quantity: String,
    pub price: Option<String>,
    pub time_in_force: Option<TimeInForce>,
    pub position_side: Option<PositionSide>,
    pub new_client_order_id: Option<String>,
}

impl PlaceOrderRequest {
    pub fn market(
        symbol: impl Into<String>,
        side: OrderSide,
        quantity: impl Into<String>,
    ) -> Self {
        Self {
            symbol: symbol.into().to_ascii_uppercase(),
            side,
            order_type: OrderType::Market,
            quantity: quantity.into(),
            price: None,
            time_in_force: None,
            position_side: None,
            new_client_order_id: None,
        }
    }

    pub fn limit(
        symbol: impl Into<String>,
        side: OrderSide,
        quantity: impl Into<String>,
        price: impl Into<String>,
        time_in_force: TimeInForce,
    ) -> Self {
        Self {
            symbol: symbol.into().to_ascii_uppercase(),
            side,
            order_type: OrderType::Limit,
            quantity: quantity.into(),
            price: Some(price.into()),
            time_in_force: Some(time_in_force),
            position_side: None,
            new_client_order_id: None,
        }
    }

    pub fn with_position_side(mut self, position_side: PositionSide) -> Self {
        self.position_side = Some(position_side);
        self
    }
}

/// Binance REST acknowledgement for a placed USDM order.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlaceOrderResponse {
    pub symbol: String,
    pub order_id: i64,
    pub client_order_id: String,
    pub update_time: u64,
    pub price: String,
    pub orig_qty: String,
    pub executed_qty: String,
    pub status: String,
    pub side: String,
    #[serde(rename = "type")]
    pub order_type: String,
    pub position_side: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct BinanceApiErrorBody {
    pub code: i64,
    pub msg: String,
}

#[derive(Debug, Error)]
pub enum OrderBuilderError {
    #[error("invalid side: {0}")]
    InvalidSide(String),
    #[error("invalid time in force: {0}")]
    InvalidTimeInForce(String),
    #[error("invalid position side: {0}")]
    InvalidPositionSide(String),
    #[error("limit order requires price")]
    LimitRequiresPrice,
    #[error("market order must not include price")]
    MarketWithPrice,
}

#[derive(Debug, Error)]
pub enum TransportError {
    #[error("http transport: {0}")]
    Http(String),
}

#[derive(Debug, Error)]
pub enum PlaceOrderError {
    #[error(transparent)]
    Builder(#[from] OrderBuilderError),
    #[error(transparent)]
    Transport(#[from] TransportError),
    #[error("binance api error {code}: {msg}")]
    Api { code: i64, msg: String },
    #[error("unexpected response status {status}: {body}")]
    UnexpectedStatus { status: u16, body: String },
    #[error("failed to parse response: {0}")]
    Parse(String),
}

/// HTTP response from an order transport.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpResponse {
    pub status: u16,
    pub body: String,
}

/// Pluggable HTTP transport for tests and production.
pub trait OrderTransport: Send + Sync {
    fn post_form(
        &self,
        url: &str,
        api_key: &str,
        body: &str,
    ) -> impl std::future::Future<Output = Result<HttpResponse, TransportError>> + Send;
}

/// Production transport using `tokio` + native TLS (no `reqwest` dependency).
#[derive(Clone, Debug, Default)]
pub struct NativeTlsTransport;

impl NativeTlsTransport {
    pub fn new() -> Self {
        Self
    }
}

impl OrderTransport for NativeTlsTransport {
    async fn post_form(
        &self,
        url: &str,
        api_key: &str,
        body: &str,
    ) -> Result<HttpResponse, TransportError> {
        post_form_native_tls(url, api_key, body).await
    }
}

async fn post_form_native_tls(
    url: &str,
    api_key: &str,
    body: &str,
) -> Result<HttpResponse, TransportError> {
    let (host, port, path) = parse_https_url(url)?;
    let connector = native_tls::TlsConnector::builder()
        .build()
        .map_err(|e| TransportError::Http(e.to_string()))?;
    let connector = tokio_native_tls::TlsConnector::from(connector);

    let tcp = TcpStream::connect((host.as_str(), port))
        .await
        .map_err(|e| TransportError::Http(e.to_string()))?;
    let mut stream = connector
        .connect(host.as_str(), tcp)
        .await
        .map_err(|e| TransportError::Http(e.to_string()))?;

    let request = format!(
        "POST {path} HTTP/1.1\r\n\
         Host: {host}\r\n\
         X-MBX-APIKEY: {api_key}\r\n\
         Content-Type: application/x-www-form-urlencoded\r\n\
         Content-Length: {len}\r\n\
         Connection: close\r\n\
         \r\n\
         {body}",
        path = path,
        host = host,
        api_key = api_key,
        len = body.len(),
        body = body,
    );
    stream
        .write_all(request.as_bytes())
        .await
        .map_err(|e| TransportError::Http(e.to_string()))?;
    stream
        .shutdown()
        .await
        .map_err(|e| TransportError::Http(e.to_string()))?;

    let mut raw = Vec::new();
    stream
        .read_to_end(&mut raw)
        .await
        .map_err(|e| TransportError::Http(e.to_string()))?;

    parse_http_response(&raw)
}

fn parse_https_url(url: &str) -> Result<(String, u16, String), TransportError> {
    let rest = url
        .strip_prefix("https://")
        .ok_or_else(|| TransportError::Http("only https URLs are supported".into()))?;
    let (authority, path) = match rest.split_once('/') {
        Some((authority, path)) => (authority, format!("/{path}")),
        None => (rest, "/".into()),
    };
    let (host, port) = match authority.split_once(':') {
        Some((host, port)) => (
            host.to_string(),
            port.parse()
                .map_err(|_| TransportError::Http("invalid port".into()))?,
        ),
        None => (authority.to_string(), 443),
    };
    Ok((host, port, path))
}

fn parse_http_response(raw: &[u8]) -> Result<HttpResponse, TransportError> {
    let text = std::str::from_utf8(raw).map_err(|e| TransportError::Http(e.to_string()))?;
    let (header_block, body) = text
        .split_once("\r\n\r\n")
        .ok_or_else(|| TransportError::Http("malformed http response".into()))?;
    let status_line = header_block
        .lines()
        .next()
        .ok_or_else(|| TransportError::Http("missing status line".into()))?;
    let status = status_line
        .split_whitespace()
        .nth(1)
        .ok_or_else(|| TransportError::Http("missing status code".into()))?
        .parse::<u16>()
        .map_err(|_| TransportError::Http("invalid status code".into()))?;
    Ok(HttpResponse {
        status,
        body: body.to_string(),
    })
}

/// Signed USDM order client.
#[derive(Clone, Debug)]
pub struct UsdmOrderClient<T: OrderTransport> {
    pub credentials: ApiCredentials,
    pub base_url: String,
    transport: T,
}

impl<T: OrderTransport> UsdmOrderClient<T> {
    pub fn new(credentials: ApiCredentials, transport: T) -> Self {
        Self {
            credentials,
            base_url: REST_BASE_URL.into(),
            transport,
        }
    }

    pub fn with_base_url(mut self, base_url: impl Into<String>) -> Self {
        self.base_url = base_url.into();
        self
    }

    pub fn build_signed_form_params(
        &self,
        request: &PlaceOrderRequest,
    ) -> Result<BTreeMap<String, String>, OrderBuilderError> {
        build_signed_form_params(&self.credentials.secret_key, request)
    }

    pub async fn place_order(
        &self,
        request: PlaceOrderRequest,
    ) -> Result<PlaceOrderResponse, PlaceOrderError> {
        let params = self.build_signed_form_params(&request)?;
        let body = signed_params_payload(&params);
        let url = format!("{}/fapi/v1/order", self.base_url.trim_end_matches('/'));

        let response = self
            .transport
            .post_form(&url, &self.credentials.api_key, &body)
            .await?;

        parse_place_order_response(response)
    }
}

/// Build unsigned order params (before timestamp/signature).
pub fn build_order_params(
    request: &PlaceOrderRequest,
) -> Result<BTreeMap<String, String>, OrderBuilderError> {
    let mut params = BTreeMap::new();
    params.insert("symbol".into(), request.symbol.to_ascii_uppercase());
    params.insert("side".into(), request.side.as_str().into());
    params.insert("type".into(), request.order_type.as_str().into());
    params.insert("quantity".into(), request.quantity.clone());

    let position_side = request
        .position_side
        .unwrap_or(PositionSide::Both)
        .as_str();
    params.insert("positionSide".into(), position_side.into());

    match request.order_type {
        OrderType::Market => {
            if request.price.is_some() {
                return Err(OrderBuilderError::MarketWithPrice);
            }
        }
        OrderType::Limit => {
            let price = request
                .price
                .as_ref()
                .ok_or(OrderBuilderError::LimitRequiresPrice)?;
            params.insert("price".into(), price.clone());
            let tif = request
                .time_in_force
                .unwrap_or(TimeInForce::Gtc)
                .as_str();
            params.insert("timeInForce".into(), tif.into());
        }
    }

    if let Some(client_id) = &request.new_client_order_id {
        params.insert("newClientOrderId".into(), client_id.clone());
    }

    Ok(params)
}

/// Sign order params with timestamp and HMAC-SHA256 signature.
pub fn build_signed_form_params(
    secret_key: &str,
    request: &PlaceOrderRequest,
) -> Result<BTreeMap<String, String>, OrderBuilderError> {
    let mut params = build_order_params(request)?;
    params.insert("timestamp".into(), current_timestamp_ms().to_string());
    let payload = signed_params_payload(&params);
    let signature = sign_hmac_sha256_hex(secret_key, &payload);
    params.insert("signature".into(), signature);
    Ok(params)
}

pub fn parse_place_order_response(
    response: HttpResponse,
) -> Result<PlaceOrderResponse, PlaceOrderError> {
    if response.status == 200 {
        return serde_json::from_str(&response.body)
            .map_err(|e| PlaceOrderError::Parse(e.to_string()));
    }

    if let Ok(api_err) = serde_json::from_str::<BinanceApiErrorBody>(&response.body) {
        return Err(PlaceOrderError::Api {
            code: api_err.code,
            msg: api_err.msg,
        });
    }

    Err(PlaceOrderError::UnexpectedStatus {
        status: response.status,
        body: response.body,
    })
}

#[cfg(test)]
pub(crate) mod mock_transport {
    use super::*;
    use std::sync::{Arc, Mutex};

    #[derive(Clone)]
    pub struct MockTransport {
        pub response: HttpResponse,
        pub captured: Arc<Mutex<Vec<CapturedRequest>>>,
    }

    impl Default for MockTransport {
        fn default() -> Self {
            Self::with_response(HttpResponse {
                status: 200,
                body: String::new(),
            })
        }
    }

    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct CapturedRequest {
        pub url: String,
        pub api_key: String,
        pub body: String,
    }

    impl MockTransport {
        pub fn with_response(response: HttpResponse) -> Self {
            Self {
                response,
                captured: Arc::new(Mutex::new(Vec::new())),
            }
        }
    }

    impl OrderTransport for MockTransport {
        async fn post_form(
            &self,
            url: &str,
            api_key: &str,
            body: &str,
        ) -> Result<HttpResponse, TransportError> {
            self.captured
                .lock()
                .expect("mock transport lock")
                .push(CapturedRequest {
                    url: url.into(),
                    api_key: api_key.into(),
                    body: body.into(),
                });
            Ok(self.response.clone())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::sign_hmac_sha256_hex;

    const FIXTURE_ACK: &str = include_str!("../tests/fixtures/place_order_ack.json");
    const FIXTURE_REJECT: &str = include_str!("../tests/fixtures/place_order_reject.json");

    #[test]
    fn market_order_params_include_position_side_default_both() {
        let request = PlaceOrderRequest::market("btcusdt", OrderSide::Buy, "0.01");
        let params = build_order_params(&request).unwrap();
        assert_eq!(params.get("symbol"), Some(&"BTCUSDT".into()));
        assert_eq!(params.get("side"), Some(&"BUY".into()));
        assert_eq!(params.get("type"), Some(&"MARKET".into()));
        assert_eq!(params.get("quantity"), Some(&"0.01".into()));
        assert_eq!(params.get("positionSide"), Some(&"BOTH".into()));
        assert!(!params.contains_key("price"));
        assert!(!params.contains_key("timeInForce"));
    }

    #[test]
    fn limit_order_params_include_price_tif_and_position_side() {
        let request = PlaceOrderRequest::limit(
            "ETHUSDT",
            OrderSide::Sell,
            "1.5",
            "2500.00",
            TimeInForce::Ioc,
        )
        .with_position_side(PositionSide::Short);
        let params = build_order_params(&request).unwrap();
        assert_eq!(params.get("price"), Some(&"2500.00".into()));
        assert_eq!(params.get("timeInForce"), Some(&"IOC".into()));
        assert_eq!(params.get("positionSide"), Some(&"SHORT".into()));
    }

    #[test]
    fn signed_payload_matches_hmac_fixture() {
        let request = PlaceOrderRequest::limit(
            "BTCUSDT",
            OrderSide::Buy,
            "0.01",
            "50000.00",
            TimeInForce::Gtc,
        )
        .with_position_side(PositionSide::Long);
        let mut params = build_order_params(&request).unwrap();
        params.insert("timestamp".into(), "1747385641636".into());
        let payload = signed_params_payload(&params);
        let signature = sign_hmac_sha256_hex("test-secret", &payload);
        assert_eq!(
            payload,
            "positionSide=LONG&price=50000.00&quantity=0.01&side=BUY&symbol=BTCUSDT&timeInForce=GTC&timestamp=1747385641636&type=LIMIT"
        );
        assert!(!signature.is_empty());
    }

    #[test]
    fn parse_ack_fixture() {
        let response = HttpResponse {
            status: 200,
            body: FIXTURE_ACK.into(),
        };
        let ack = parse_place_order_response(response).unwrap();
        assert_eq!(ack.symbol, "BTCUSDT");
        assert_eq!(ack.order_id, 8_886_774);
        assert_eq!(ack.status, "NEW");
        assert_eq!(ack.position_side, "LONG");
    }

    #[test]
    fn parse_reject_fixture() {
        let response = HttpResponse {
            status: 400,
            body: FIXTURE_REJECT.into(),
        };
        let err = parse_place_order_response(response).unwrap_err();
        assert!(matches!(
            err,
            PlaceOrderError::Api {
                code: -1111,
                msg: _
            }
        ));
    }

    #[tokio::test]
    async fn place_order_posts_signed_form_to_rest_endpoint() {
        use mock_transport::MockTransport;

        let transport = MockTransport::with_response(HttpResponse {
            status: 200,
            body: FIXTURE_ACK.into(),
        });
        let client = UsdmOrderClient::new(
            ApiCredentials {
                api_key: "test-key".into(),
                secret_key: "test-secret".into(),
            },
            transport.clone(),
        );

        let request = PlaceOrderRequest::limit(
            "BTCUSDT",
            OrderSide::Buy,
            "0.01",
            "50000.00",
            TimeInForce::Gtc,
        )
        .with_position_side(PositionSide::Long);
        let ack = client.place_order(request).await.unwrap();
        assert_eq!(ack.order_id, 8_886_774);

        let captured = transport.captured.lock().unwrap();
        assert_eq!(captured.len(), 1);
        assert_eq!(captured[0].url, "https://fapi.binance.com/fapi/v1/order");
        assert_eq!(captured[0].api_key, "test-key");
        assert!(captured[0].body.contains("symbol=BTCUSDT"));
        assert!(captured[0].body.contains("side=BUY"));
        assert!(captured[0].body.contains("type=LIMIT"));
        assert!(captured[0].body.contains("price=50000.00"));
        assert!(captured[0].body.contains("timeInForce=GTC"));
        assert!(captured[0].body.contains("positionSide=LONG"));
        assert!(captured[0].body.contains("timestamp="));
        assert!(captured[0].body.contains("signature="));
    }
}
