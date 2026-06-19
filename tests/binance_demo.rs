//! Binance **demo/testnet** integration tests (spot + USDM).
//!
//! ## Offline (default)
//!
//! ```text
//! cargo test --test binance_demo
//! ```
//!
//! Runs no network I/O. Live demo tests are `#[ignore]`.
//!
//! ## Demo hosts
//!
//! | Venue | REST | User-data WebSocket |
//! |-------|------|---------------------|
//! | Spot | `https://demo-api.binance.com/api` | `wss://demo-ws-api.binance.com/ws-api/v3` (signed `userDataStream.subscribe.signature`) |
//! | Spot market | — | `wss://demo-stream.binance.com/ws` |
//! | USDM | `https://demo-fapi.binance.com` | `wss://fstream.binancefuture.com/private/ws/<listenKey>` |
//! | USDM market | — | `wss://fstream.binancefuture.com` |
//!
//! Production depth providers map from [`src/providers/depth/binance/spot.rs`](../src/providers/depth/binance/spot.rs);
//! demo hosts are wired in [`binance-spot-exec`](../crates/binance-spot-exec) and [`binance-usdm-exec`](../crates/binance-usdm-exec).
//!
//! ## Live demo (opt-in via `--ignored`)
//!
//! 1. `cp .env.example .env`
//! 2. Set `DEMO_BINANCE_KEY` and `DEMO_BINANCE_SECRET` in `.env`
//! 3. Optional: `TROLLY_DEMO_SYMBOL` (default `BTCUSDT`)
//! 4. Run ignored tests:
//!
//! ```text
//! cargo test --test binance_demo -- --ignored
//! ```
//!
//! Uses Binance **demo** endpoints only — never production API keys.
//!
//! ## Follow-on checklist (not blocking WP-017; after WP-014 / WP-015)
//!
//! - [ ] Spot: `POST` demo order → assert `executionReport` on user-data stream reconciles placement
//! - [ ] USDM: `POST` demo order on `demo-fapi.binance.com` → assert `ORDER_TRADE_UPDATE` reconcile

use std::time::Duration;

use binance_spot_exec::{
    parse_user_data_message, ApiCredentials, BinanceSpotUserStream, SpotUserEvent,
    SPOT_DEMO_REST_BASE_URL, spot_depth_rest_url,
};
use binance_usdm_exec::{
    parse_user_events, ApiCredentials as UsdmCredentials, ListenKeyClient, UsdmExecUpdate,
    UsdmUserDataStream, USDM_DEMO_REST_BASE_URL, usdm_depth_rest_url,
};
use futures_util::{SinkExt, StreamExt};
use http::Uri;
use serde_json::Value;
use trolly_stream::{Message, VenueEndpoints};
use tokio::time::timeout;
use tokio_tungstenite::connect_async_tls_with_config;

const EVENT_WAIT: Duration = Duration::from_secs(20);

fn load_demo_credentials() -> Option<(ApiCredentials, UsdmCredentials)> {
    let api_key = std::env::var("DEMO_BINANCE_KEY")
        .ok()
        .filter(|v| !v.trim().is_empty())?;
    let secret_key = std::env::var("DEMO_BINANCE_SECRET")
        .ok()
        .filter(|v| !v.trim().is_empty())?;
    let spot = ApiCredentials {
        api_key: api_key.clone(),
        secret_key: secret_key.clone(),
    };
    let usdm = UsdmCredentials {
        api_key,
        secret_key,
    };
    Some((spot, usdm))
}

fn demo_symbol() -> String {
    std::env::var("TROLLY_DEMO_SYMBOL").unwrap_or_else(|_| "BTCUSDT".into())
}

fn skip_if_demo_disabled() -> bool {
    dotenvy::dotenv().ok();
    if load_demo_credentials().is_none() {
        eprintln!("skip: DEMO_BINANCE_KEY / DEMO_BINANCE_SECRET missing or empty");
        return true;
    }
    false
}

async fn fetch_json(url: &str) -> Value {
    let response = reqwest::get(url)
        .await
        .unwrap_or_else(|e| panic!("REST GET {url} failed: {e}"));
    let status = response.status();
    let body = response
        .text()
        .await
        .unwrap_or_else(|e| panic!("REST body read failed: {e}"));
    assert!(
        status.is_success(),
        "REST {url} returned {status}: {body}"
    );
    serde_json::from_str(&body).unwrap_or_else(|e| panic!("REST JSON parse failed: {e}"))
}

async fn connect_ws(url: &str) -> tokio_tungstenite::WebSocketStream<
    tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
> {
    let uri: Uri = url.parse().expect("valid websocket uri");
    let (socket, _) = connect_async_tls_with_config(uri, None, false, None)
        .await
        .unwrap_or_else(|e| panic!("websocket connect {url} failed: {e}"));
    socket
}

/// Spot demo: REST depth snapshot + signed user-data subscribe on demo WS API.
#[tokio::test]
#[ignore = "live Binance spot demo; set DEMO_BINANCE_KEY/DEMO_BINANCE_SECRET in .env and run with --ignored"]
async fn spot_demo_rest_depth_and_user_data_stream() {
    if skip_if_demo_disabled() {
        return;
    }
    let (credentials, _) = load_demo_credentials().expect("credentials checked");
    let symbol = demo_symbol();

    let depth_url = spot_depth_rest_url(SPOT_DEMO_REST_BASE_URL, &symbol, 100);
    let depth = fetch_json(&depth_url).await;
    let update_id = depth
        .get("lastUpdateId")
        .and_then(Value::as_u64)
        .unwrap_or_else(|| panic!("depth missing lastUpdateId: {depth}"));
    assert!(update_id > 0, "empty demo spot depth for {symbol}");

    let stream = BinanceSpotUserStream::demo(credentials);
    let ws_url = stream.websocket_url();
    let mut socket = connect_ws(&ws_url).await;
    socket
        .send(Message::Text(stream.subscribe_request_json().into()).into())
        .await
        .expect("send subscribe");

    let mut subscribed = false;
    let mut parsed_events = Vec::new();
    let wait = timeout(EVENT_WAIT, async {
        while let Some(frame) = socket.next().await {
            let frame = frame.expect("websocket frame");
            let Message::Text(text) = Message::from(frame) else {
                continue;
            };
            let value: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
            if value.get("result").is_some() && value.get("id").is_some() {
                subscribed = true;
                continue;
            }
            if let Ok(Some(event)) = parse_user_data_message(Message::Text(text)) {
                parsed_events.push(event);
            }
        }
    })
    .await;

    assert!(subscribed, "demo spot user-data subscribe ack not received");

    if parsed_events.is_empty() {
        eprintln!(
            "skip assertions: demo spot account idle — no executionReport/account events in {:?} (subscribe ok, depth ok)",
            EVENT_WAIT
        );
        return;
    }

    let has_exec_or_account = parsed_events.iter().any(|event| {
        matches!(
            event,
            SpotUserEvent::ExecutionReport(_)
                | SpotUserEvent::OutboundAccountPosition(_)
                | SpotUserEvent::BalanceUpdate(_)
        )
    });
    assert!(
        has_exec_or_account,
        "received user-data frames but none were execution/account events: {parsed_events:?}"
    );

    if wait.is_err() && parsed_events.is_empty() {
        eprintln!("note: websocket wait timed out after subscribe ack");
    }
}

/// USDM demo: REST depth + listenKey lifecycle + user-data stream parsing.
#[tokio::test]
#[ignore = "live Binance USDM demo; set DEMO_BINANCE_KEY/DEMO_BINANCE_SECRET in .env and run with --ignored"]
async fn usdm_demo_rest_depth_listen_key_and_user_data_stream() {
    if skip_if_demo_disabled() {
        return;
    }
    let (_, credentials) = load_demo_credentials().expect("credentials checked");
    let symbol = demo_symbol();

    let depth_url = usdm_depth_rest_url(USDM_DEMO_REST_BASE_URL, &symbol, 100);
    let depth = fetch_json(&depth_url).await;
    let update_id = depth
        .get("lastUpdateId")
        .and_then(Value::as_u64)
        .or_else(|| {
            depth
                .get("lastUpdateId")
                .and_then(|v| v.as_i64())
                .map(|v| v as u64)
        })
        .unwrap_or_else(|| panic!("depth missing lastUpdateId: {depth}"));
    assert!(update_id > 0, "empty demo USDM depth for {symbol}");

    let listen_client = ListenKeyClient::demo(credentials.clone());
    let listen_key = listen_client
        .create()
        .await
        .expect("demo listenKey create");
    assert!(!listen_key.is_empty(), "empty listenKey");

    let stream = UsdmUserDataStream::demo(&listen_key)
        .with_events_filter("ORDER_TRADE_UPDATE/ACCOUNT_UPDATE");
    let mut socket = connect_ws(&stream.websocket_url()).await;

    listen_client
        .keepalive()
        .await
        .expect("demo listenKey keepalive");

    let mut parsed_events = Vec::new();
    let wait = timeout(EVENT_WAIT, async {
        while let Some(frame) = socket.next().await {
            let frame = frame.expect("websocket frame");
            let Message::Text(text) = Message::from(frame) else {
                continue;
            };
            if let Ok(events) = parse_user_events(Message::Text(text)) {
                parsed_events.extend(events);
            }
        }
    })
    .await;

    let _ = listen_client.close().await;

    if parsed_events.is_empty() {
        eprintln!(
            "skip assertions: demo USDM account idle — no ORDER_TRADE_UPDATE/ACCOUNT_UPDATE in {:?} (listenKey lifecycle ok, depth ok)",
            EVENT_WAIT
        );
        return;
    }

    let has_order_or_account = parsed_events.iter().any(|event| {
        matches!(
            event,
            UsdmExecUpdate::OrderTrade(_) | UsdmExecUpdate::BalanceChange(_)
        ) || matches!(event, UsdmExecUpdate::PositionChange(_))
    });
    assert!(
        has_order_or_account,
        "received user-data frames but none were ORDER_TRADE_UPDATE/ACCOUNT_UPDATE: {parsed_events:?}"
    );

    if wait.is_err() && parsed_events.is_empty() {
        eprintln!("note: websocket wait timed out with no user events");
    }
}
