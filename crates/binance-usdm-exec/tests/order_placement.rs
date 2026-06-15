//! Recorded HTTP tests for signed USDM order placement (no live keys).

use binance_usdm_exec::{
    ApiCredentials, OrderSide, PositionSide, UsdmOrderClient, UsdmOrderRequest, UsdmRestEgress,
};
use trolly_strategy::{OutboundMessage, StreamEgress};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const ORDER_ACK_JSON: &str = r#"{
  "orderId": 8886774,
  "symbol": "BTCUSDT",
  "status": "NEW",
  "clientOrderId": "test-client-id",
  "price": "100000.0",
  "avgPrice": "0.00000",
  "origQty": "0.010",
  "executedQty": "0",
  "cumQty": "0",
  "cumQuote": "0",
  "timeInForce": "GTC",
  "type": "LIMIT",
  "reduceOnly": false,
  "closePosition": false,
  "side": "BUY",
  "positionSide": "LONG",
  "stopPrice": "0",
  "workingType": "CONTRACT_PRICE",
  "priceProtect": false,
  "origType": "LIMIT",
  "priceMatch": "NONE",
  "selfTradePreventionMode": "NONE",
  "goodTillDate": 0,
  "updateTime": 1587727187525
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
        .and(path("/fapi/v1/order"))
        .respond_with(ResponseTemplate::new(200).set_body_string(ORDER_ACK_JSON))
        .mount(&server)
        .await;

    let client = UsdmOrderClient::new(server.uri(), test_credentials());
    let response = client
        .place_order(UsdmOrderRequest::limit_with_position(
            "BTCUSDT",
            OrderSide::Buy,
            "0.01",
            "100000",
            PositionSide::Long,
        ))
        .await
        .expect("place order");

    assert_eq!(response.symbol, "BTCUSDT");
    assert_eq!(response.order_id, 8_886_774);
    assert_eq!(response.status, "NEW");
    assert_eq!(response.side, "BUY");
    assert_eq!(response.position_side, "LONG");

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
    assert!(body.contains("positionSide=LONG"));
    assert!(body.contains("timestamp="));
    assert!(body.contains("signature="));
}

#[tokio::test]
async fn usdm_rest_egress_dispatches_order_request() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/fapi/v1/order"))
        .respond_with(ResponseTemplate::new(200).set_body_string(ORDER_ACK_JSON))
        .mount(&server)
        .await;

    let mut egress = UsdmRestEgress::new(server.uri(), test_credentials());
    egress
        .dispatch(OutboundMessage::order_request(
            "BTCUSDT",
            "BUY",
            "0.01",
            Some("100000".into()),
            Some("LONG".into()),
        ))
        .expect("egress dispatch");

    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 1);
}

#[tokio::test]
async fn usdm_rest_egress_market_order_via_sync_dispatch() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/fapi/v1/order"))
        .respond_with(ResponseTemplate::new(200).set_body_string(ORDER_ACK_JSON))
        .mount(&server)
        .await;

    let mut egress = UsdmRestEgress::new(server.uri(), test_credentials());
    egress
        .dispatch(OutboundMessage::order_request(
            "BTCUSDT",
            "SELL",
            "0.01",
            None,
            None,
        ))
        .expect("market egress dispatch");

    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 1);
    let body = std::str::from_utf8(&requests[0].body).unwrap();
    assert!(body.contains("type=MARKET"));
    assert!(!body.contains("price="));
    assert!(!body.contains("positionSide="));
}

#[tokio::test]
async fn place_order_surfaces_api_errors() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/fapi/v1/order"))
        .respond_with(
            ResponseTemplate::new(400)
                .set_body_string(r#"{"code":-2019,"msg":"Margin is insufficient."}"#),
        )
        .mount(&server)
        .await;

    let client = UsdmOrderClient::new(server.uri(), test_credentials());
    let err = client
        .place_order(UsdmOrderRequest::market("BTCUSDT", OrderSide::Buy, "0.01"))
        .await
        .expect_err("expected api error");

    let binance_usdm_exec::OrderError::Api { status, body } = err else {
        panic!("expected api error variant");
    };
    assert_eq!(status, 400);
    assert!(body.contains("Margin is insufficient"));
}
