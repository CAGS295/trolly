//! Binance **demo / testnet** integration tests for spot and USDM execution crates.
//!
//! ## Offline (default)
//!
//! ```text
//! cargo test --test binance_demo_integration
//! ```
//!
//! No network; live tests are `#[ignore]`.
//!
//! ## Live demo (opt-in)
//!
//! 1. `cp .env.example .env`
//! 2. Set `RUN_BINANCE_DEMO_INTEGRATION=1`, `DEMO_BINANCE_KEY`, and `DEMO_BINANCE_SECRET` in `.env`
//! 3. `cargo test --test binance_demo_integration -- --ignored`
//!
//! Optional: `TROLLY_DEMO_SYMBOL` (default `BTCUSDT`).
//!
//! ## Demo base URLs
//!
//! | Venue | REST | User-data WebSocket |
//! |-------|------|---------------------|
//! | Spot | `https://demo-api.binance.com/api` | `wss://demo-ws-api.binance.com/ws-api/v3` (signed `userDataStream.subscribe.signature`) |
//! | Spot market streams | — | `wss://demo-stream.binance.com/ws` |
//! | USDM | `https://demo-fapi.binance.com` | `wss://fstream.binancefuture.com/private/ws/<listenKey>` (listenKey from demo REST) |
//! | USDM market streams | — | `wss://fstream.binancefuture.com` |
//!
//! Production hosts are mapped in [`src/providers/depth/binance/spot.rs`](../src/providers/depth/binance/spot.rs)
//! and the [`binance-spot-exec`](../crates/binance-spot-exec) / [`binance-usdm-exec`](../crates/binance-usdm-exec) crates.
//!
//! ## Follow-on (WP-014 / WP-015 — not blocking)
//!
//! - [ ] Spot: demo `POST /api/v3/order` → `executionReport` on user-data stream round-trip
//! - [ ] USDM: demo `POST /fapi/v1/order` → `ORDER_TRADE_UPDATE` round-trip
//! - [ ] Reconcile REST ack status vs stream fill/reject for both venues

use std::time::Duration;

use binance_spot_exec::{
    parse_user_data_message, ApiCredentials, BinanceSpotUserStream, SpotUserEvent,
    demo_depth_rest_url as spot_demo_depth_url,
};
use binance_usdm_exec::{
    parse_user_events, ApiCredentials as UsdmCredentials, ListenKeyClient, UsdmExecUpdate,
    UsdmUserDataStream, demo_depth_rest_url as usdm_demo_depth_url,
};
use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use trolly_stream::{Message, VenueEndpoints};
use tokio_tungstenite::{connect_async, tungstenite::Message as WsMessage};

const USER_EVENT_WAIT: Duration = Duration::from_secs(8);

#[derive(Debug, Deserialize)]
struct DepthSnapshot {
    #[serde(rename = "lastUpdateId")]
    last_update_id: u64,
    bids: Vec<[String; 2]>,
    asks: Vec<[String; 2]>,
}

fn demo_integration_enabled() -> bool {
    std::env::var("RUN_BINANCE_DEMO_INTEGRATION")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false)
}

fn demo_credentials() -> Option<(String, String)> {
    let key = std::env::var("DEMO_BINANCE_KEY")
        .ok()
        .filter(|v| !v.trim().is_empty())?;
    let secret = std::env::var("DEMO_BINANCE_SECRET")
        .ok()
        .filter(|v| !v.trim().is_empty())?;
    Some((key, secret))
}

fn demo_symbol() -> String {
    std::env::var("TROLLY_DEMO_SYMBOL").unwrap_or_else(|_| "BTCUSDT".into())
}

fn skip_unless_demo_live() -> Option<(String, String)> {
    dotenvy::dotenv().ok();
    if !demo_integration_enabled() {
        eprintln!("skip: RUN_BINANCE_DEMO_INTEGRATION not enabled");
        return None;
    }
    let creds = demo_credentials();
    if creds.is_none() {
        eprintln!("skip: DEMO_BINANCE_KEY / DEMO_BINANCE_SECRET missing or empty");
    }
    creds
}

async fn fetch_depth(url: &str) -> DepthSnapshot {
    let response = reqwest::get(url)
        .await
        .unwrap_or_else(|e| panic!("REST GET {url} failed: {e}"));
    response
        .error_for_status()
        .expect("REST status")
        .json()
        .await
        .expect("REST JSON")
}

async fn recv_ws_text(
    read: &mut futures_util::stream::SplitStream<
        tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
    >,
    timeout: Duration,
) -> Option<String> {
    match tokio::time::timeout(timeout, async {
        while let Some(msg) = read.next().await {
            match msg {
                Ok(WsMessage::Text(text)) => return Some(text.to_string()),
                Ok(WsMessage::Ping(payload)) => {
                    // caller should pong via write half; ignore here
                    let _ = payload;
                }
                Ok(WsMessage::Close(_)) => return None,
                Ok(_) => {}
                Err(e) => panic!("websocket read error: {e}"),
            }
        }
        None
    })
    .await
    {
        Ok(text) => text,
        Err(_) => None,
    }
}

/// Spot demo REST depth + signed user-data subscribe on demo WS API.
#[tokio::test]
#[ignore = "live Binance spot demo; set RUN_BINANCE_DEMO_INTEGRATION=1 in .env"]
async fn spot_demo_rest_depth_and_user_data_ws() {
    let Some((api_key, secret_key)) = skip_unless_demo_live() else {
        return;
    };

    let symbol = demo_symbol();
    let depth_url = spot_demo_depth_url(&symbol);
    let depth = fetch_depth(&depth_url).await;
    assert!(depth.last_update_id > 0, "empty spot demo depth book");
    assert!(!depth.bids.is_empty() && !depth.asks.is_empty(), "spot demo book sides");

    let stream = BinanceSpotUserStream::demo(ApiCredentials {
        api_key,
        secret_key,
    });
    let ws_url = stream.websocket_url();
    let (ws, _) = connect_async(&ws_url)
        .await
        .unwrap_or_else(|e| panic!("connect {ws_url} failed: {e}"));
    let (mut write, mut read) = ws.split();

    let subscribe = stream.subscribe_request_json();
    write
        .send(WsMessage::Text(subscribe.into()))
        .await
        .expect("send subscribe");

    let mut saw_subscribe_ack = false;
    let mut saw_execution_or_account = false;

    let deadline = tokio::time::Instant::now() + USER_EVENT_WAIT;
    while tokio::time::Instant::now() < deadline {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        let Some(text) = recv_ws_text(&mut read, remaining.min(Duration::from_secs(2))).await
        else {
            break;
        };

        if parse_user_data_message(Message::Text(text.clone().into()))
            .ok()
            .flatten()
            .is_none()
            && text.contains("\"result\"")
            && text.contains("\"id\"")
        {
            saw_subscribe_ack = true;
            continue;
        }

        let Ok(Some(event)) = parse_user_data_message(Message::Text(text.into())) else {
            continue;
        };
        if matches!(
            event,
            SpotUserEvent::ExecutionReport(_)
                | SpotUserEvent::OutboundAccountPosition(_)
                | SpotUserEvent::BalanceUpdate(_)
        ) {
            saw_execution_or_account = true;
            break;
        }
    }

    assert!(
        saw_subscribe_ack,
        "expected userDataStream.subscribe.signature ack from demo WS API"
    );

    if !saw_execution_or_account {
        eprintln!(
            "skip user-event assertions: demo spot account idle (no executionReport / account events in {USER_EVENT_WAIT:?})"
        );
    }
}

/// USDM demo REST depth + listenKey lifecycle + user-data stream parsing.
#[tokio::test]
#[ignore = "live Binance USDM demo; set RUN_BINANCE_DEMO_INTEGRATION=1 in .env"]
async fn usdm_demo_rest_depth_listen_key_and_user_data_ws() {
    let Some((api_key, secret_key)) = skip_unless_demo_live() else {
        return;
    };

    let symbol = demo_symbol();
    let depth_url = usdm_demo_depth_url(&symbol);
    let depth = fetch_depth(&depth_url).await;
    assert!(depth.last_update_id > 0, "empty USDM demo depth book");
    assert!(!depth.bids.is_empty() && !depth.asks.is_empty(), "USDM demo book sides");

    let listen_client = ListenKeyClient::new(
        UsdmCredentials {
            api_key: api_key.clone(),
            secret_key: secret_key.clone(),
        },
        binance_usdm_exec::DEMO_REST_BASE,
    );
    let listen_key = listen_client
        .create()
        .await
        .expect("create listenKey on demo-fapi");
    assert!(
        !listen_key.listen_key.is_empty(),
        "demo listenKey should be non-empty"
    );

    listen_client
        .keepalive()
        .await
        .expect("keepalive listenKey on demo-fapi");

    let stream = UsdmUserDataStream::demo(&listen_key.listen_key)
        .with_events_filter("ORDER_TRADE_UPDATE/ACCOUNT_UPDATE");
    let ws_url = stream.websocket_url();
    let (ws, _) = connect_async(&ws_url)
        .await
        .unwrap_or_else(|e| panic!("connect {ws_url} failed: {e}"));
    let (_write, mut read) = ws.split();

    let mut saw_order_or_account = false;
    let deadline = tokio::time::Instant::now() + USER_EVENT_WAIT;
    while tokio::time::Instant::now() < deadline {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        let Some(text) = recv_ws_text(&mut read, remaining.min(Duration::from_secs(2))).await
        else {
            break;
        };

        let events = parse_user_events(Message::Text(text.into())).unwrap_or_else(|err| {
            panic!("unexpected USDM demo user-data payload: {err}");
        });
        if events.is_empty() {
            continue;
        }

        for event in events {
            if matches!(
                event,
                UsdmExecUpdate::OrderTrade(_) | UsdmExecUpdate::BalanceChange(_)
            ) || matches!(event, UsdmExecUpdate::PositionChange(_))
            {
                saw_order_or_account = true;
                break;
            }
        }
        if saw_order_or_account {
            break;
        }
    }

    if !saw_order_or_account {
        eprintln!(
            "skip user-event assertions: demo USDM account idle (no ORDER_TRADE_UPDATE / ACCOUNT_UPDATE in {USER_EVENT_WAIT:?})"
        );
    }
}
