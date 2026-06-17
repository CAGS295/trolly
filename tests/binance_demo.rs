//! Binance **demo/testnet** integration tests (spot + USDM).
//!
//! ## Offline (default)
//!
//! ```text
//! cargo test --test binance_demo
//! ```
//!
//! Ignored tests are not compiled into the default run path for network I/O, but `cargo test`
//! without `--ignored` still lists them as ignored and does not contact Binance.
//!
//! ## Demo integration (opt-in)
//!
//! 1. `cp .env.example .env`
//! 2. Set `RUN_BINANCE_DEMO_INTEGRATION=1`, `DEMO_BINANCE_KEY`, and `DEMO_BINANCE_SECRET` in `.env`
//! 3. `cargo test --test binance_demo -- --ignored`
//!
//! Optional: `TROLLY_DEMO_SYMBOL` (default `BTCUSDT`).
//!
//! ### Demo base URLs
//!
//! | Venue | REST | User-data WebSocket |
//! |-------|------|---------------------|
//! | Spot | `https://demo-api.binance.com/api` | `wss://demo-ws-api.binance.com/ws-api/v3` (signed `userDataStream.subscribe.signature`) |
//! | Spot market streams | — | `wss://demo-stream.binance.com/ws` |
//! | USDM | `https://demo-fapi.binance.com` | `wss://fstream.binancefuture.com/private/ws/<listenKey>` (listen key from demo REST) |
//! | USDM market streams | — | `wss://fstream.binancefuture.com` |
//!
//! Never use production API keys. Geo/network restrictions (e.g. HTTP 451) cause a clean skip.
//!
//! ## Follow-on (WP-014 / WP-015, optional — not blocking)
//!
//! - [ ] Spot: place limit order on demo REST → reconcile `executionReport` on user-data stream
//! - [ ] USDM: place limit order on `demo-fapi` → reconcile `ORDER_TRADE_UPDATE` on user-data stream

use std::time::Duration;

use binance_spot_exec::{
    parse_user_data_message, ApiCredentials, BinanceSpotUserStream, SpotUserEvent,
};
use binance_usdm_exec::{
    parse_user_events, ApiCredentials as UsdmCredentials, UsdmExecUpdate, UsdmListenKeyClient,
    UsdmUserDataStream,
};
use futures_util::{SinkExt, StreamExt};
use serde_json::Value;
use tokio_tungstenite::{connect_async_tls_with_config, tungstenite::Message};
use trolly_stream::VenueEndpoints;

const USER_DATA_WAIT: Duration = Duration::from_secs(12);

fn demo_integration_enabled() -> bool {
    dotenvy::dotenv().ok();
    std::env::var("RUN_BINANCE_DEMO_INTEGRATION")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false)
}

fn demo_credentials() -> Option<(String, String)> {
    let key = std::env::var("DEMO_BINANCE_KEY")
        .ok()
        .filter(|v| !v.is_empty())?;
    let secret = std::env::var("DEMO_BINANCE_SECRET")
        .ok()
        .filter(|v| !v.is_empty())?;
    Some((key, secret))
}

fn demo_symbol() -> String {
    std::env::var("TROLLY_DEMO_SYMBOL").unwrap_or_else(|_| "BTCUSDT".into())
}

fn skip_unless_demo_ready(test_name: &str) -> Option<(String, String)> {
    if !demo_integration_enabled() {
        eprintln!("skip: RUN_BINANCE_DEMO_INTEGRATION not enabled ({test_name})");
        return None;
    }
    match demo_credentials() {
        Some(creds) => Some(creds),
        None => {
            eprintln!("skip: DEMO_BINANCE_KEY or DEMO_BINANCE_SECRET missing ({test_name})");
            None
        }
    }
}

fn geo_or_restricted(status: reqwest::StatusCode, body: &str) -> bool {
    status.as_u16() == 451
        || body.to_ascii_lowercase().contains("restricted")
        || body.to_ascii_lowercase().contains("service unavailable from")
}

async fn fetch_json(url: &str) -> Result<Value, String> {
    let response = reqwest::get(url)
        .await
        .map_err(|e| format!("REST GET failed: {e}"))?;
    let status = response.status();
    let body = response
        .text()
        .await
        .map_err(|e| format!("REST body read failed: {e}"))?;
    if geo_or_restricted(status, &body) {
        return Err(format!("geo/network restricted (status {}): {body}", status));
    }
    if !status.is_success() {
        return Err(format!("REST status {}: {body}", status));
    }
    serde_json::from_str(&body).map_err(|e| format!("REST JSON parse failed: {e}"))
}

async fn connect_ws(url: &str) -> Result<
    tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
    String,
> {
    connect_async_tls_with_config(url, None, false, None)
        .await
        .map(|(stream, _)| stream)
        .map_err(|e| format!("websocket connect failed: {e}"))
}

fn is_spot_subscribe_ack(text: &str) -> bool {
    let Ok(value) = serde_json::from_str::<Value>(text) else {
        return false;
    };
    value.get("id").is_some() && value.get("result").is_some()
}

/// Spot demo REST depth + signed user-data subscribe on demo WS API.
#[tokio::test]
#[ignore = "live Binance demo; set RUN_BINANCE_DEMO_INTEGRATION=1 in .env"]
async fn spot_demo_rest_depth_and_user_data_stream() {
    let Some((api_key, secret_key)) = skip_unless_demo_ready("spot_demo_rest_depth_and_user_data_stream")
    else {
        return;
    };

    let symbol = demo_symbol();
    let depth_url = BinanceSpotUserStream::demo_rest_depth_url(&symbol);
    let depth = match fetch_json(&depth_url).await {
        Ok(json) => json,
        Err(reason) => {
            eprintln!("skip: {reason}");
            return;
        }
    };
    let last_update_id = depth
        .get("lastUpdateId")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    assert!(last_update_id > 0, "empty spot demo depth for {symbol}");

    let stream = BinanceSpotUserStream::demo(ApiCredentials {
        api_key,
        secret_key,
    });
    let ws_url = stream.websocket_url();
    let mut ws = match connect_ws(&ws_url).await {
        Ok(ws) => ws,
        Err(reason) => {
            eprintln!("skip: {reason}");
            return;
        }
    };

    let subscribe = stream.subscribe_request_json();
    if ws.send(Message::Text(subscribe.into())).await.is_err() {
        eprintln!("skip: failed to send spot demo user-data subscribe");
        return;
    }

    let deadline = tokio::time::Instant::now() + USER_DATA_WAIT;
    let mut subscribe_ack = false;
    let mut execution_reports = 0usize;
    let mut account_events = 0usize;

    while tokio::time::Instant::now() < deadline {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        let frame = match tokio::time::timeout(remaining, ws.next()).await {
            Ok(Some(Ok(Message::Text(text)))) => text,
            Ok(Some(Ok(_))) => continue,
            Ok(Some(Err(e))) => {
                eprintln!("skip: spot demo websocket read error: {e}");
                return;
            }
            Ok(None) | Err(_) => break,
        };

        if is_spot_subscribe_ack(&frame) {
            subscribe_ack = true;
            continue;
        }

        match parse_user_data_message(Message::Text(frame)) {
            Ok(Some(SpotUserEvent::ExecutionReport(_))) => execution_reports += 1,
            Ok(Some(
                SpotUserEvent::OutboundAccountPosition(_)
                | SpotUserEvent::BalanceUpdate(_)
                | SpotUserEvent::ExternalLockUpdate(_),
            )) => account_events += 1,
            Ok(Some(_)) => {}
            Ok(None) => {}
            Err(err) => panic!("spot demo user-data parse error: {err}"),
        }
    }

    assert!(
        subscribe_ack,
        "expected userDataStream.subscribe.signature ack on demo WS API"
    );

    if execution_reports == 0 && account_events == 0 {
        eprintln!(
            "skip: demo spot account idle — no executionReport or account events in {:?}",
            USER_DATA_WAIT
        );
        return;
    }

    if execution_reports > 0 {
        eprintln!("parsed {execution_reports} executionReport event(s) from demo spot stream");
    }
    if account_events > 0 {
        eprintln!("parsed {account_events} account event(s) from demo spot stream");
    }
}

/// USDM demo REST depth + listenKey lifecycle + user-data stream parsing.
#[tokio::test]
#[ignore = "live Binance demo; set RUN_BINANCE_DEMO_INTEGRATION=1 in .env"]
async fn usdm_demo_rest_depth_listen_key_and_user_data_stream() {
    let Some((api_key, secret_key)) =
        skip_unless_demo_ready("usdm_demo_rest_depth_listen_key_and_user_data_stream")
    else {
        return;
    };

    let symbol = demo_symbol();
    let depth_url = UsdmUserDataStream::demo_rest_depth_url(&symbol);
    let depth = match fetch_json(&depth_url).await {
        Ok(json) => json,
        Err(reason) => {
            eprintln!("skip: {reason}");
            return;
        }
    };
    let last_update_id = depth
        .get("lastUpdateId")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let book_ok = last_update_id > 0
        || depth
            .get("bids")
            .and_then(Value::as_array)
            .is_some_and(|b| !b.is_empty());
    assert!(book_ok, "empty USDM demo depth for {symbol}");

    let listen_client = UsdmListenKeyClient::demo(UsdmCredentials {
        api_key: api_key.clone(),
        secret_key,
    });

    let listen_key = match listen_client.create_listen_key().await {
        Ok(key) => key,
        Err(err) => {
            let msg = err.to_string();
            if msg.contains("451") || msg.to_ascii_lowercase().contains("restricted") {
                eprintln!("skip: geo/network restricted creating USDM demo listenKey: {msg}");
                return;
            }
            panic!("USDM demo listenKey create failed: {err}");
        }
    };
    assert!(!listen_key.is_empty(), "empty listenKey from demo-fapi");

    listen_client
        .keepalive_listen_key()
        .await
        .expect("USDM demo listenKey keepalive");

    let user_stream = UsdmUserDataStream::demo(&listen_key)
        .with_events_filter("ORDER_TRADE_UPDATE/ACCOUNT_UPDATE");
    let ws_url = user_stream.websocket_url();
    let mut ws = match connect_ws(&ws_url).await {
        Ok(ws) => ws,
        Err(reason) => {
            let _ = listen_client.close_listen_key().await;
            eprintln!("skip: {reason}");
            return;
        }
    };

    let deadline = tokio::time::Instant::now() + USER_DATA_WAIT;
    let mut order_trade_updates = 0usize;
    let mut account_updates = 0usize;

    while tokio::time::Instant::now() < deadline {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        let frame = match tokio::time::timeout(remaining, ws.next()).await {
            Ok(Some(Ok(Message::Text(text)))) => text,
            Ok(Some(Ok(_))) => continue,
            Ok(Some(Err(e))) => {
                let _ = listen_client.close_listen_key().await;
                eprintln!("skip: USDM demo websocket read error: {e}");
                return;
            }
            Ok(None) | Err(_) => break,
        };

        let events = match parse_user_events(Message::Text(frame)) {
            Ok(events) => events,
            Err(err) => {
                let _ = listen_client.close_listen_key().await;
                panic!("USDM demo user-data parse error: {err}");
            }
        };

        for event in events {
            match event {
                UsdmExecUpdate::OrderTrade(_) => order_trade_updates += 1,
                UsdmExecUpdate::BalanceChange(_) | UsdmExecUpdate::PositionChange(_) => {
                    account_updates += 1;
                }
                UsdmExecUpdate::ListenKeyExpired => {
                    let _ = listen_client.close_listen_key().await;
                    panic!("unexpected listenKeyExpired on demo USDM stream");
                }
                UsdmExecUpdate::MarginCall(_) => {}
            }
        }
    }

    listen_client
        .close_listen_key()
        .await
        .expect("USDM demo listenKey close");

    if order_trade_updates == 0 && account_updates == 0 {
        eprintln!(
            "skip: demo USDM account idle — no ORDER_TRADE_UPDATE or ACCOUNT_UPDATE in {:?}",
            USER_DATA_WAIT
        );
        return;
    }

    if order_trade_updates > 0 {
        eprintln!(
            "parsed {order_trade_updates} ORDER_TRADE_UPDATE event(s) from demo USDM stream"
        );
    }
    if account_updates > 0 {
        eprintln!("parsed {account_updates} ACCOUNT_UPDATE row(s) from demo USDM stream");
    }
}
