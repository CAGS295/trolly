//! Mock HTTP integration tests for signed spot order placement.

use std::sync::{Arc, Mutex};

use binance_spot_exec::{
    build_order, ApiCredentials, NewOrderRequest, OrderSide, OrderBuildError, SpotExecEgress,
    SpotOrderClient, TimeInForce,
};
use trolly_strategy::{OutboundMessage, StreamEgress};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

fn test_credentials() -> ApiCredentials {
    ApiCredentials {
        api_key: "test-key".into(),
        secret_key: "test-secret".into(),
    }
}

async fn mock_order_server(captured: Arc<Mutex<Option<String>>>) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let body = include_str!("fixtures/place_order_response.json");

    tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut buf = vec![0u8; 8192];
        let n = stream.read(&mut buf).await.unwrap();
        let request = String::from_utf8_lossy(&buf[..n]).to_string();
        *captured.lock().unwrap() = Some(request);

        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        stream.write_all(response.as_bytes()).await.unwrap();
    });

    format!("http://{addr}")
}

#[tokio::test]
async fn place_order_posts_signed_limit_request() {
    let captured = Arc::new(Mutex::new(None));
    let base_url = mock_order_server(Arc::clone(&captured)).await;

    let client = SpotOrderClient::with_base_url(base_url, test_credentials());
    let order = NewOrderRequest::limit(
        "BTCUSDT",
        OrderSide::Buy,
        "0.01",
        "100.0",
        TimeInForce::Gtc,
    )
    .timestamp_ms(1_700_000_000_000);

    let response = client.place_order(&order).await.unwrap();
    assert_eq!(response.order_id, 28);
    assert_eq!(response.status, "NEW");

    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    let request = captured.lock().unwrap().clone().expect("request captured");
    assert!(request.contains("POST /api/v3/order?"));
    assert!(request.contains("symbol=BTCUSDT"));
    assert!(request.contains("side=BUY"));
    assert!(request.contains("type=LIMIT"));
    assert!(request.contains("quantity=0.01"));
    assert!(request.contains("price=100.0"));
    assert!(request.contains("timeInForce=GTC"));
    assert!(request.contains("timestamp=1700000000000"));
    assert!(request.contains("signature="));
    assert!(request.to_ascii_lowercase().contains("x-mbx-apikey: test-key"));
}

#[tokio::test]
async fn place_order_posts_signed_market_request() {
    let captured = Arc::new(Mutex::new(None));
    let base_url = mock_order_server(Arc::clone(&captured)).await;

    let client = SpotOrderClient::with_base_url(base_url, test_credentials());
    let order = NewOrderRequest::market("ETHBTC", OrderSide::Sell, "1.0")
        .timestamp_ms(1_700_000_000_001);

    client.place_order(&order).await.unwrap();

    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    let request = captured.lock().unwrap().clone().expect("request captured");
    assert!(request.contains("type=MARKET"));
    assert!(!request.contains("timeInForce="));
    assert!(!request.contains("price="));
}

#[test]
fn build_order_market_and_limit() {
    let market = build_order("BTCUSDT", "buy", "1", None, None).unwrap();
    assert_eq!(market.order_type, binance_spot_exec::OrderType::Market);

    let limit = build_order("BTCUSDT", "SELL", "1", Some("99"), Some("IOC")).unwrap();
    assert_eq!(limit.time_in_force, Some(TimeInForce::Ioc));
}

#[test]
fn egress_dispatches_strategy_order_request() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let captured = Arc::new(Mutex::new(None));
    let base_url = rt.block_on(mock_order_server(Arc::clone(&captured)));

    let client = SpotOrderClient::with_base_url(base_url, test_credentials());
    let mut egress = SpotExecEgress::new(client);

    egress
        .dispatch(OutboundMessage::OrderRequest {
            symbol: "BTCUSDT".into(),
            side: "BUY".into(),
            qty: "0.01".into(),
            price: Some("100.0".into()),
        })
        .unwrap();

    std::thread::sleep(std::time::Duration::from_millis(50));
    let request = captured.lock().unwrap().clone().expect("request captured");
    assert!(request.contains("POST /api/v3/order?"));
    assert!(request.contains("signature="));
}

#[test]
fn build_order_rejects_invalid_side() {
    let err = build_order("BTCUSDT", "HODL", "1", None, None).unwrap_err();
    assert!(matches!(err, OrderBuildError::InvalidSide(_)));
}
