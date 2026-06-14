//! Mock HTTP tests for signed spot order placement.

use binance_spot_exec::{
    ApiCredentials, NewOrderRequest, OrderSide, SpotOrderClient, SpotOrderEgress, TimeInForce,
};
use trolly_strategy::{OutboundMessage, StreamEgress};
use wiremock::matchers::{body_string_contains, header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test]
async fn place_limit_order_posts_signed_form_body() {
    let mock = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/v3/order"))
        .and(header("X-MBX-APIKEY", "test-key"))
        .and(body_string_contains("symbol=BTCUSDT"))
        .and(body_string_contains("side=BUY"))
        .and(body_string_contains("type=LIMIT"))
        .and(body_string_contains("timeInForce=GTC"))
        .and(body_string_contains("timestamp="))
        .and(body_string_contains("signature="))
        .respond_with(ResponseTemplate::new(200).set_body_raw(
            include_str!("fixtures/new_order_response.json"),
            "application/json",
        ))
        .expect(1)
        .mount(&mock)
        .await;

    let client = SpotOrderClient::with_rest_base(
        ApiCredentials {
            api_key: "test-key".into(),
            secret_key: "test-secret".into(),
        },
        mock.uri(),
    );

    let request = NewOrderRequest::limit("BTCUSDT", OrderSide::Buy, "0.01", "100", TimeInForce::Gtc);
    let ack = client.place_order(&request).await.expect("place order");
    assert_eq!(ack.symbol, "BTCUSDT");
    assert_eq!(ack.order_id, 28);
    assert_eq!(ack.status, "NEW");
}

#[tokio::test]
async fn place_order_surfaces_binance_api_error() {
    let mock = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/v3/order"))
        .respond_with(ResponseTemplate::new(400).set_body_json(serde_json::json!({
            "code": -1013,
            "msg": "Filter failure: LOT_SIZE"
        })))
        .mount(&mock)
        .await;

    let client = SpotOrderClient::with_rest_base(
        ApiCredentials {
            api_key: "test-key".into(),
            secret_key: "test-secret".into(),
        },
        mock.uri(),
    );

    let request = NewOrderRequest::market("BTCUSDT", OrderSide::Buy, "0.01");
    let err = client
        .place_order(&request)
        .await
        .expect_err("expected api error");
    assert!(err.to_string().contains("-1013"));
}

#[test]
fn spot_order_egress_dispatches_strategy_order_request() {
    let rt = tokio::runtime::Runtime::new().expect("runtime");
    let (client, _mock) = rt.block_on(async {
        let mock = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/v3/order"))
            .and(body_string_contains("type=MARKET"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(
                include_str!("fixtures/new_order_response.json"),
                "application/json",
            ))
            .expect(1)
            .mount(&mock)
            .await;

        let client = SpotOrderClient::with_rest_base(
            ApiCredentials {
                api_key: "test-key".into(),
                secret_key: "test-secret".into(),
            },
            mock.uri(),
        );
        (client, mock)
    });

    let mut egress = SpotOrderEgress::with_client(client);
    egress
        .dispatch(OutboundMessage::OrderRequest {
            symbol: "BTCUSDT".into(),
            side: "BUY".into(),
            qty: "0.01".into(),
            price: None,
            position_side: None,
        })
        .expect("dispatch order");
}
