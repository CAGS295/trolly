//! Binance demo integration tests (spot + USDM authenticated streams).
//!
//! Uses Binance **demo/testnet** hosts only — never production keys.
//!
//! ## Demo base URLs
//!
//! **Spot** ([demo general info](https://developers.binance.com/docs/binance-spot-api-docs/demo-mode/general-info)):
//! - REST: `https://demo-api.binance.com/api`
//! - WS API: `wss://demo-ws-api.binance.com/ws-api/v3`
//! - Market streams: `wss://demo-stream.binance.com/ws`
//!
//! **USDM** ([derivatives docs](https://developers.binance.com/docs/derivatives/)):
//! - REST: `https://demo-fapi.binance.com`
//! - Market streams: `wss://fstream.binancefuture.com`
//! - User-data: `POST /fapi/v1/listenKey` on demo REST +
//!   `wss://fstream.binancefuture.com/private/ws/<listenKey>`
//!
//! ## Offline (default)
//!
//! ```text
//! cargo test --test binance_demo_integration
//! ```
//!
//! No network I/O; ignored live tests are not compiled into the default path beyond `#[ignore]`.
//!
//! ## Opt-in demo integration
//!
//! 1. `cp .env.example .env`
//! 2. Set `RUN_BINANCE_DEMO_INTEGRATION=1`, `DEMO_BINANCE_KEY`, `DEMO_BINANCE_SECRET`
//! 3. `cargo test --test binance_demo_integration -- --ignored`
//!
//! Optional: `TROLLY_DEMO_SYMBOL` (default `BTCUSDT`). If the flag is unset, `0`, or keys are
//! missing, live tests print `skip` and return without contacting the network.
//!
//! ## Follow-on (not blocking WP-017)
//!
//! - [ ] Spot: demo limit order place → `executionReport` on user-data stream
//! - [ ] USDM: demo limit order place → `ORDER_TRADE_UPDATE` reconcile round-trip

use std::time::Duration;

use binance_spot_exec::{
    parse_user_data_message, ApiCredentials, BinanceSpotUserStream, SpotUserEvent,
    demo_depth_rest_url,
};
use binance_usdm_exec::{
    parse_user_events, ApiCredentials as UsdmCredentials, ListenKeyClient, UsdmExecUpdate,
    UsdmUserDataStream, demo_depth_rest_url as usdm_demo_depth_rest_url,
    DEMO_REST_BASE_URL as USDM_DEMO_REST_BASE_URL,
};
use futures_util::{SinkExt, StreamExt};
use lob::LimitOrderBook;
use tokio_tungstenite::{
    connect_async_tls_with_config,
    tungstenite::{Error as WsError, Message},
};
use trolly_stream::VenueEndpoints;

const EVENT_WAIT: Duration = Duration::from_secs(8);

fn demo_integration_enabled() -> bool {
    dotenvy::dotenv().ok();
    std::env::var("RUN_BINANCE_DEMO_INTEGRATION")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false)
}

fn demo_symbol() -> String {
    std::env::var("TROLLY_DEMO_SYMBOL").unwrap_or_else(|_| "BTCUSDT".into())
}

fn demo_credentials() -> Option<(String, String)> {
    let key = std::env::var("DEMO_BINANCE_KEY")
        .ok()
        .filter(|value| !value.trim().is_empty())?;
    let secret = std::env::var("DEMO_BINANCE_SECRET")
        .ok()
        .filter(|value| !value.trim().is_empty())?;
    Some((key, secret))
}

fn skip_unless_demo_enabled() -> Option<(String, String)> {
    if !demo_integration_enabled() {
        eprintln!("skip: RUN_BINANCE_DEMO_INTEGRATION not enabled");
        return None;
    }
    match demo_credentials() {
        Some(creds) => Some(creds),
        None => {
            eprintln!("skip: DEMO_BINANCE_KEY or DEMO_BINANCE_SECRET missing");
            None
        }
    }
}

fn is_geo_or_network_blocked(err: &impl std::fmt::Display) -> bool {
    let message = err.to_string().to_lowercase();
    message.contains("connection refused")
        || message.contains("timed out")
        || message.contains("timeout")
        || message.contains("dns")
        || message.contains("network")
        || message.contains("451")
        || message.contains("403 forbidden")
        || message.contains("403")
}

async fn fetch_demo_depth(url: &str) -> Result<LimitOrderBook, reqwest::Error> {
    reqwest::get(url)
        .await?
        .error_for_status()?
        .json()
        .await
}

/// Spot demo: REST depth snapshot + signed user-data subscribe on demo WS API.
#[tokio::test]
#[ignore = "live Binance spot demo; set RUN_BINANCE_DEMO_INTEGRATION=1 in .env"]
async fn binance_spot_demo_depth_and_user_stream() {
    let Some((api_key, secret_key)) = skip_unless_demo_enabled() else {
        return;
    };
    let symbol = demo_symbol();

    let depth_url = demo_depth_rest_url(&symbol);
    let book = match fetch_demo_depth(&depth_url).await {
        Ok(book) => book,
        Err(err) if is_geo_or_network_blocked(&err) => {
            eprintln!("skip: demo spot REST blocked or unreachable ({err})");
            return;
        }
        Err(err) => panic!("demo spot REST depth failed: {err}"),
    };
    assert!(book.update_id > 0, "empty demo spot book for {symbol}");

    let stream = BinanceSpotUserStream::demo(ApiCredentials {
        api_key,
        secret_key,
    });
    let ws_url = stream.websocket_url();
    let (mut ws, _) = match connect_async_tls_with_config(&ws_url, None, false, None).await {
        Ok(pair) => pair,
        Err(WsError::Io(err)) if is_geo_or_network_blocked(&err) => {
            eprintln!("skip: demo spot WS API blocked or unreachable ({err})");
            return;
        }
        Err(err) => panic!("demo spot WS connect failed: {err}"),
    };

    ws.send(Message::Text(stream.subscribe_request_json().into()))
        .await
        .expect("send spot user-data subscribe");

    let mut subscribed = false;
    let mut parsed_account_event = false;
    let deadline = tokio::time::Instant::now() + EVENT_WAIT;

    while tokio::time::Instant::now() < deadline {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        let next = tokio::time::timeout(remaining, ws.next()).await;
        let Ok(result) = next else { break };
        let Some(Ok(Message::Text(text))) = result else {
            continue;
        };

        if let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) {
            if value.get("result").is_some() && value.get("id").is_some() {
                subscribed = true;
                continue;
            }
        }

        match parse_user_data_message(Message::Text(text)) {
            Ok(Some(SpotUserEvent::ExecutionReport(_)))
            | Ok(Some(SpotUserEvent::OutboundAccountPosition(_)))
            | Ok(Some(SpotUserEvent::BalanceUpdate(_))) => {
                parsed_account_event = true;
                break;
            }
            Ok(Some(_)) | Ok(None) => {}
            Err(err) => panic!("failed to parse spot user-data payload: {err}"),
        }
    }

    assert!(subscribed, "expected demo spot user-data subscribe ack");
    if !parsed_account_event {
        eprintln!(
            "skip: demo spot account idle — no executionReport/account events within {:?}",
            EVENT_WAIT
        );
    }
}

/// USDM demo: REST depth + listenKey lifecycle on demo-fapi + user-data stream.
#[tokio::test]
#[ignore = "live Binance USDM demo; set RUN_BINANCE_DEMO_INTEGRATION=1 in .env"]
async fn binance_usdm_demo_depth_listen_key_and_user_stream() {
    let Some((api_key, secret_key)) = skip_unless_demo_enabled() else {
        return;
    };
    let symbol = demo_symbol();
    let credentials = UsdmCredentials {
        api_key: api_key.clone(),
        secret_key: secret_key.clone(),
    };

    let depth_url = usdm_demo_depth_rest_url(&symbol);
    let book = match fetch_demo_depth(&depth_url).await {
        Ok(book) => book,
        Err(err) if is_geo_or_network_blocked(&err) => {
            eprintln!("skip: demo USDM REST blocked or unreachable ({err})");
            return;
        }
        Err(err) => panic!("demo USDM REST depth failed: {err}"),
    };
    assert!(book.update_id > 0, "empty demo USDM book for {symbol}");

    let listen_client = ListenKeyClient::new(USDM_DEMO_REST_BASE_URL, credentials.clone());
    let listen_key = match listen_client.create().await {
        Ok(key) => key,
        Err(err) if is_geo_or_network_blocked(&err) => {
            eprintln!("skip: demo USDM listenKey create blocked or unreachable ({err})");
            return;
        }
        Err(err) => panic!("demo USDM listenKey create failed: {err}"),
    };
    assert!(!listen_key.is_empty(), "empty listenKey from demo-fapi");

    listen_client
        .keepalive()
        .await
        .expect("demo USDM listenKey keepalive");

    let stream = UsdmUserDataStream::demo(&listen_key)
        .with_events_filter("ORDER_TRADE_UPDATE/ACCOUNT_UPDATE");
    let ws_url = stream.websocket_url();
    let (mut ws, _) = match connect_async_tls_with_config(&ws_url, None, false, None).await {
        Ok(pair) => pair,
        Err(WsError::Io(err)) if is_geo_or_network_blocked(&err) => {
            let _ = listen_client.close().await;
            eprintln!("skip: demo USDM user-data WS blocked or unreachable ({err})");
            return;
        }
        Err(err) => {
            let _ = listen_client.close().await;
            panic!("demo USDM user-data WS connect failed: {err}");
        }
    };

    let mut parsed_execution_event = false;
    let deadline = tokio::time::Instant::now() + EVENT_WAIT;

    while tokio::time::Instant::now() < deadline {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        let next = tokio::time::timeout(remaining, ws.next()).await;
        let Ok(result) = next else { break };
        let Some(Ok(Message::Text(text))) = result else {
            continue;
        };

        match parse_user_events(Message::Text(text)) {
            Ok(events) if events.iter().any(|event| {
                matches!(
                    event,
                    UsdmExecUpdate::OrderTrade(_) | UsdmExecUpdate::BalanceChange(_)
                        | UsdmExecUpdate::PositionChange(_)
                )
            }) => {
                parsed_execution_event = true;
                break;
            }
            Ok(_) => {}
            Err(err) => panic!("failed to parse USDM user-data payload: {err}"),
        }
    }

    listen_client
        .close()
        .await
        .expect("demo USDM listenKey close");

    if !parsed_execution_event {
        eprintln!(
            "skip: demo USDM account idle — no ORDER_TRADE_UPDATE/ACCOUNT_UPDATE within {:?}",
            EVENT_WAIT
        );
    }
}
