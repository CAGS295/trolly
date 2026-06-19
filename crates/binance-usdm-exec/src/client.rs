use std::fmt;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::Deserialize;

use crate::endpoints::ApiCredentials;
use crate::order::{NewOrderRequest, NewOrderResponse, OrderBuildError};

pub const DEFAULT_REST_API_URL: &str = "https://fapi.binance.com";

/// Transport hook for `POST /fapi/v1/order` (mocked in tests; minimal HTTPS in production).
pub trait OrderHttpTransport: Send + Sync {
    fn post_order(
        &self,
        url: &str,
        api_key: &str,
        form_body: &str,
    ) -> Result<Vec<u8>, OrderTransportError>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OrderTransportError {
    pub message: String,
}

impl fmt::Display for OrderTransportError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for OrderTransportError {}

/// Signed REST client for Binance USDM order placement.
pub struct UsdmOrderClient<T: OrderHttpTransport = NativeTlsOrderTransport> {
    credentials: ApiCredentials,
    base_url: String,
    transport: T,
}

impl UsdmOrderClient<NativeTlsOrderTransport> {
    pub fn new(credentials: ApiCredentials) -> Self {
        Self::with_base_url(credentials, DEFAULT_REST_API_URL)
    }

    pub fn with_base_url(credentials: ApiCredentials, base_url: impl Into<String>) -> Self {
        Self {
            credentials,
            base_url: base_url.into(),
            transport: NativeTlsOrderTransport::default(),
        }
    }
}

impl<T: OrderHttpTransport> UsdmOrderClient<T> {
    pub fn with_transport(
        credentials: ApiCredentials,
        base_url: impl Into<String>,
        transport: T,
    ) -> Self {
        Self {
            credentials,
            base_url: base_url.into(),
            transport,
        }
    }

    pub fn credentials(&self) -> &ApiCredentials {
        &self.credentials
    }

    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    pub fn order_url(&self) -> String {
        format!("{}/fapi/v1/order", self.base_url.trim_end_matches('/'))
    }

    /// Place an order via signed REST. Fills/rejects reconcile via user-data `ORDER_TRADE_UPDATE`.
    pub fn place_order(&self, request: &NewOrderRequest) -> Result<NewOrderResponse, OrderError> {
        let form_body = request.signed_form_body(&self.credentials.secret_key)?;
        let bytes = self
            .transport
            .post_order(
                &self.order_url(),
                &self.credentials.api_key,
                &form_body,
            )
            .map_err(OrderError::Transport)?;
        parse_order_response(&bytes)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OrderError {
    Build(OrderBuildError),
    Transport(OrderTransportError),
    Api { code: i64, msg: String },
    InvalidResponse(String),
    UnsupportedEgress(String),
}

impl fmt::Display for OrderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Build(err) => write!(f, "{err}"),
            Self::Transport(err) => write!(f, "transport error: {err}"),
            Self::Api { code, msg } => write!(f, "binance api error {code}: {msg}"),
            Self::InvalidResponse(msg) => write!(f, "invalid order response: {msg}"),
            Self::UnsupportedEgress(kind) => write!(f, "usdm egress cannot dispatch {kind}"),
        }
    }
}

impl std::error::Error for OrderError {}

impl From<OrderBuildError> for OrderError {
    fn from(value: OrderBuildError) -> Self {
        Self::Build(value)
    }
}

#[derive(Debug, Deserialize)]
struct BinanceApiError {
    code: i64,
    msg: String,
}

fn parse_order_response(bytes: &[u8]) -> Result<NewOrderResponse, OrderError> {
    if let Ok(api_err) = serde_json::from_slice::<BinanceApiError>(bytes) {
        return Err(OrderError::Api {
            code: api_err.code,
            msg: api_err.msg,
        });
    }
    serde_json::from_slice(bytes).map_err(|err| OrderError::InvalidResponse(err.to_string()))
}

/// Minimal HTTPS POST using native TLS (no extra HTTP crate dependencies).
#[derive(Default)]
pub struct NativeTlsOrderTransport;

impl OrderHttpTransport for NativeTlsOrderTransport {
    fn post_order(
        &self,
        url: &str,
        api_key: &str,
        form_body: &str,
    ) -> Result<Vec<u8>, OrderTransportError> {
        https_post_form(url, api_key, form_body)
    }
}

fn https_post_form(
    url: &str,
    api_key: &str,
    form_body: &str,
) -> Result<Vec<u8>, OrderTransportError> {
    let parsed = url::Url::parse(url).map_err(|err| OrderTransportError {
        message: err.to_string(),
    })?;
    let host = parsed
        .host_str()
        .ok_or_else(|| OrderTransportError {
            message: "missing host in order url".into(),
        })?
        .to_string();
    let port = parsed.port().unwrap_or(443);
    let path = parsed.path();

    let connector = native_tls::TlsConnector::builder()
        .build()
        .map_err(|err| OrderTransportError {
            message: err.to_string(),
        })?;
    let tcp = TcpStream::connect((host.as_str(), port)).map_err(|err| OrderTransportError {
        message: err.to_string(),
    })?;
    tcp.set_read_timeout(Some(Duration::from_secs(30)))
        .map_err(|err| OrderTransportError {
            message: err.to_string(),
        })?;
    tcp.set_write_timeout(Some(Duration::from_secs(30)))
        .map_err(|err| OrderTransportError {
            message: err.to_string(),
        })?;
    let mut tls = connector.connect(&host, tcp).map_err(|err| OrderTransportError {
        message: err.to_string(),
    })?;

    let request = format!(
        "POST {path} HTTP/1.1\r\n\
         Host: {host}\r\n\
         X-MBX-APIKEY: {api_key}\r\n\
         Content-Type: application/x-www-form-urlencoded\r\n\
         Content-Length: {}\r\n\
         Connection: close\r\n\
         \r\n\
         {form_body}",
        form_body.len()
    );
    tls.write_all(request.as_bytes()).map_err(|err| OrderTransportError {
        message: err.to_string(),
    })?;

    let mut response = Vec::new();
    tls.read_to_end(&mut response).map_err(|err| OrderTransportError {
        message: err.to_string(),
    })?;

    let body = split_http_body(&response).ok_or_else(|| OrderTransportError {
        message: "invalid HTTP response".into(),
    })?;
    if !response.starts_with(b"HTTP/1.1 200") && !response.starts_with(b"HTTP/1.0 200") {
        if let Ok(api_err) = serde_json::from_slice::<BinanceApiError>(body) {
            return Err(OrderTransportError {
                message: format!("HTTP error: {} ({})", api_err.msg, api_err.code),
            });
        }
        return Err(OrderTransportError {
            message: format!(
                "HTTP error: {}",
                String::from_utf8_lossy(response.split(|&b| b == b'\n').next().unwrap_or(&[]))
            ),
        });
    }
    Ok(body.to_vec())
}

fn split_http_body(response: &[u8]) -> Option<&[u8]> {
    response
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .map(|idx| &response[idx + 4..])
}

/// Records outbound REST requests for unit tests (no live keys or network).
#[derive(Debug, Default)]
pub struct MockOrderTransport {
    pub requests: Arc<Mutex<Vec<(String, String, String)>>>,
    response_body: Vec<u8>,
}

impl MockOrderTransport {
    pub fn with_response(response_body: impl Into<Vec<u8>>) -> Self {
        Self {
            requests: Arc::new(Mutex::new(Vec::new())),
            response_body: response_body.into(),
        }
    }

    pub fn recorded_requests(&self) -> Vec<(String, String, String)> {
        self.requests.lock().expect("mock lock").clone()
    }
}

impl OrderHttpTransport for MockOrderTransport {
    fn post_order(
        &self,
        url: &str,
        api_key: &str,
        form_body: &str,
    ) -> Result<Vec<u8>, OrderTransportError> {
        self.requests
            .lock()
            .expect("mock lock")
            .push((url.to_string(), api_key.to_string(), form_body.to_string()));
        Ok(self.response_body.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::order::{OrderSide, PositionSide, TimeInForce};

    const MOCK_ACK: &str = r#"{
        "symbol": "BTCUSDT",
        "orderId": 28,
        "clientOrderId": "test-client-id",
        "updateTime": 1507725176595,
        "price": "0.00000000",
        "origQty": "10.00000000",
        "executedQty": "10.00000000",
        "status": "FILLED",
        "timeInForce": "GTC",
        "type": "MARKET",
        "side": "SELL",
        "positionSide": "LONG"
    }"#;

    #[test]
    fn place_order_posts_signed_form_to_rest_endpoint() {
        let transport = MockOrderTransport::with_response(MOCK_ACK.as_bytes());
        let client = UsdmOrderClient::with_transport(
            ApiCredentials {
                api_key: "test-key".into(),
                secret_key: "test-secret".into(),
            },
            "https://fapi.binance.com",
            transport,
        );

        let request = NewOrderRequest::market("BTCUSDT", OrderSide::Sell, "10")
            .with_position_side(PositionSide::Long);
        let response = client.place_order(&request).unwrap();

        assert_eq!(response.symbol, "BTCUSDT");
        assert_eq!(response.order_id, 28);
        assert_eq!(response.order_type, "MARKET");
        assert_eq!(response.side, "SELL");
        assert_eq!(response.position_side.as_deref(), Some("LONG"));

        let recorded = client.transport.recorded_requests();
        assert_eq!(recorded.len(), 1);
        let (url, api_key, body) = &recorded[0];
        assert_eq!(url, "https://fapi.binance.com/fapi/v1/order");
        assert_eq!(api_key, "test-key");
        assert!(body.contains("symbol=BTCUSDT"));
        assert!(body.contains("side=SELL"));
        assert!(body.contains("type=MARKET"));
        assert!(body.contains("positionSide=LONG"));
        assert!(body.contains("signature="));
    }

    #[test]
    fn place_order_surfaces_api_error_payload() {
        let transport = MockOrderTransport::with_response(
            br#"{"code":-1013,"msg":"Filter failure: LOT_SIZE"}"#,
        );
        let client = UsdmOrderClient::with_transport(
            ApiCredentials {
                api_key: "k".into(),
                secret_key: "s".into(),
            },
            DEFAULT_REST_API_URL,
            transport,
        );
        let request = NewOrderRequest::limit(
            "BTCUSDT",
            OrderSide::Buy,
            "0.0001",
            "1",
            TimeInForce::Gtc,
        );
        let err = client.place_order(&request).unwrap_err();
        assert_eq!(
            err,
            OrderError::Api {
                code: -1013,
                msg: "Filter failure: LOT_SIZE".into(),
            }
        );
    }

    #[test]
    fn limit_order_request_fields_are_present_in_mock_transport() {
        let transport = MockOrderTransport::with_response(MOCK_ACK.as_bytes());
        let client = UsdmOrderClient::with_transport(
            ApiCredentials {
                api_key: "k".into(),
                secret_key: "s".into(),
            },
            DEFAULT_REST_API_URL,
            transport,
        );
        let request = NewOrderRequest::limit(
            "ETHUSDT",
            OrderSide::Buy,
            "1",
            "2500",
            TimeInForce::Gtc,
        )
        .with_position_side(PositionSide::Both);
        client.place_order(&request).unwrap();
        let body = &client.transport.recorded_requests()[0].2;
        assert!(body.contains("type=LIMIT"));
        assert!(body.contains("price=2500"));
        assert!(body.contains("timeInForce=GTC"));
        assert!(body.contains("positionSide=BOTH"));
    }

    #[test]
    fn split_http_body_finds_json_payload() {
        let raw = b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n{}";
        assert_eq!(split_http_body(raw), Some(&b"{}"[..]));
    }
}
