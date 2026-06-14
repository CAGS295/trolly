//! Mock HTTP tests for signed USDM order placement.

use binance_usdm_exec::{
    ApiCredentials, NewOrderRequest, OrderSide, PositionSide, TimeInForce, UsdmOrderClient,
    UsdmOrderEgress,
};
use trolly_strategy::{OutboundMessage, StreamEgress};
use wiremock::matchers::{body_string_contains, header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test]
async fn place_limit_order_posts_signed_form_body_with_position_side() {
    let mock = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/fapi/v1/order"))
        .and(header("X-MBX-APIKEY", "test-key"))
        .and(body_string_contains("symbol=BTCUSDT"))
        .and(body_string_contains("side=BUY"))
        .and(body_string_contains("type=LIMIT"))
        .and(body_string_contains("timeInForce=GTC"))
        .and(body_string_contains("positionSide=LONG"))
        .and(body_string_contains("timestamp="))
        .and(body_string_contains("signature="))
        .respond_with(ResponseTemplate::new(200).set_body_raw(
            include_str!("fixtures/new_order_response.json"),
            "application/json",
        ))
        .expect(1)
        .mount(&mock)
        .await;

    let client = UsdmOrderClient::with_rest_base(
        ApiCredentials {
            api_key: "test-key".into(),
            secret_key: "test-secret".into(),
        },
        mock.uri(),
    );

    let request = NewOrderRequest::limit(
        "BTCUSDT",
        OrderSide::Buy,
        "0.01",
        "100",
        TimeInForce::Gtc,
    )
    .with_position_side(PositionSide::Long);
    let ack = client.place_order(&request).await.expect("place order");
    assert_eq!(ack.symbol, "BTCUSDT");
    assert_eq!(ack.order_id, 28);
    assert_eq!(ack.status, "NEW");
    assert_eq!(ack.position_side.as_deref(), Some("LONG"));
}

#[tokio::test]
async fn place_order_surfaces_binance_api_error() {
    let mock = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/fapi/v1/order"))
        .respond_with(ResponseTemplate::new(400).set_body_json(serde_json::json!({
            "code": -1111,
            "msg": "Precision is over the maximum defined for this asset."
        })))
        .mount(&mock)
        .await;

    let client = UsdmOrderClient::with_rest_base(
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
    assert!(err.to_string().contains("-1111"));
}

#[test]
fn usdm_order_egress_dispatches_strategy_order_request() {
    let rt = tokio::runtime::Runtime::new().expect("runtime");
    let (client, _mock) = rt.block_on(async {
        let mock = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/fapi/v1/order"))
            .and(body_string_contains("type=MARKET"))
            .and(body_string_contains("positionSide=SHORT"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(
                include_str!("fixtures/new_order_response.json"),
                "application/json",
            ))
            .expect(1)
            .mount(&mock)
            .await;

        let client = UsdmOrderClient::with_rest_base(
            ApiCredentials {
                api_key: "test-key".into(),
                secret_key: "test-secret".into(),
            },
            mock.uri(),
        );
        (client, mock)
    });

    let mut egress = UsdmOrderEgress::with_client(client);
    egress
        .dispatch(OutboundMessage::OrderRequest {
            symbol: "BTCUSDT".into(),
            side: "BUY".into(),
            qty: "0.01".into(),
            price: None,
            position_side: Some("SHORT".into()),
        })
        .expect("dispatch order");
}

#[tokio::test]
async fn listen_key_create_and_keepalive() {
    let mock = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/fapi/v1/listenKey"))
        .and(header("X-MBX-APIKEY", "test-key"))
        .and(body_string_contains("timestamp="))
        .and(body_string_contains("signature="))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "listenKey": "abc123"
        })))
        .expect(1)
        .mount(&mock)
        .await;

    Mock::given(method("PUT"))
        .and(path("/fapi/v1/listenKey"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({})))
        .expect(1)
        .mount(&mock)
        .await;

    use binance_usdm_exec::ListenKeyClient;

    let client = ListenKeyClient::new(
        ApiCredentials {
            api_key: "test-key".into(),
            secret_key: "test-secret".into(),
        },
        mock.uri(),
    );

    let created = client.create().await.expect("create listen key");
    assert_eq!(created.listen_key, "abc123");
    client.keepalive().await.expect("keepalive");
}
