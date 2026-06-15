//! Recorded HTTP tests for signed spot order placement (no live keys).

use binance_spot_exec::{
    ApiCredentials, OrderSide, SpotOrderClient, SpotOrderRequest, SpotRestEgress,
};
use trolly_strategy::{OutboundMessage, StreamEgress};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const ORDER_ACK_JSON: &str = r#"{
  "symbol": "BTCUSDT",
  "orderId": 28,
  "orderListId": -1,
  "clientOrderId": "test-client-id",
  "transactTime": 1507725176595,
  "price": "100000.00000000",
  "origQty": "0.01000000",
  "executedQty": "0.00000000",
  "cummulativeQuoteQty": "0.00000000",
  "status": "NEW",
  "timeInForce": "GTC",
  "type": "LIMIT",
  "side": "BUY"
}"#;

fn test_credentials() -> ApiCredentials {
    ApiCredentials {
        api_key: "test-key".into(),
        secret_key: "test-secret".into(),
    }
}

#[tokio::test]
async fn place_order_posts_signed_form_to_rest_api() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/v3/order"))
        .respond_with(ResponseTemplate::new(200).set_body_string(ORDER_ACK_JSON))
        .mount(&server)
        .await;

    let client = SpotOrderClient::new(server.uri(), test_credentials());
    let response = client
        .place_order(SpotOrderRequest::limit(
            "BTCUSDT",
            OrderSide::Buy,
            "0.01",
            "100000",
        ))
        .await
        .expect("place order");

    assert_eq!(response.symbol, "BTCUSDT");
    assert_eq!(response.order_id, 28);
    assert_eq!(response.status, "NEW");
    assert_eq!(response.side, "BUY");

    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 1);
    let request = &requests[0];
    assert_eq!(request.headers.get("X-MBX-APIKEY").unwrap(), "test-key");
    let body = std::str::from_utf8(&request.body).unwrap();
    assert!(body.contains("symbol=BTCUSDT"));
    assert!(body.contains("side=BUY"));
    assert!(body.contains("type=LIMIT"));
    assert!(body.contains("quantity=0.01"));
    assert!(body.contains("price=100000"));
    assert!(body.contains("timeInForce=GTC"));
    assert!(body.contains("timestamp="));
    assert!(body.contains("signature="));
}

#[tokio::test]
async fn spot_rest_egress_dispatches_order_request() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/v3/order"))
        .respond_with(ResponseTemplate::new(200).set_body_string(ORDER_ACK_JSON))
        .mount(&server)
        .await;

    let mut egress = SpotRestEgress::new(server.uri(), test_credentials());
    egress
        .dispatch(OutboundMessage::order_request(
            "BTCUSDT",
            "BUY",
            "0.01",
            Some("100000".into()),
        ))
        .expect("egress dispatch");

    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 1);
}

#[tokio::test]
async fn spot_rest_egress_market_order_via_sync_dispatch() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/v3/order"))
        .respond_with(ResponseTemplate::new(200).set_body_string(ORDER_ACK_JSON))
        .mount(&server)
        .await;

    let mut egress = SpotRestEgress::new(server.uri(), test_credentials());
    egress
        .dispatch(OutboundMessage::order_request(
            "BTCUSDT",
            "SELL",
            "0.01",
            None,
        ))
        .expect("market egress dispatch");

    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 1);
    let body = std::str::from_utf8(&requests[0].body).unwrap();
    assert!(body.contains("type=MARKET"));
    assert!(!body.contains("price="));
}
