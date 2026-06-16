//! Binance **demo/testnet** integration tests for spot and USDM authenticated streams.
//!
//! Uses demo hosts only — never production API keys.
//!
//! ## Demo base URLs
//!
//! | Venue | REST | WebSocket |
//! |-------|------|-----------|
//! | Spot demo | `https://demo-api.binance.com/api` | WS API `wss://demo-ws-api.binance.com/ws-api/v3`; market streams `wss://demo-stream.binance.com/ws` |
//! | USDM demo | `https://demo-fapi.binance.com` | private user-data `wss://fstream.binancefuture.com/private/ws/<listenKey>` (listen key from demo REST) |
//!
//! See [Spot demo general info](https://developers.binance.com/docs/binance-spot-api-docs/demo-mode/general-info)
//! and [USDM general info](https://developers.binance.com/docs/derivatives/usds-margined-futures/general-info).
//!
//! ## Offline (default)
//!
//! ```text
//! cargo test --workspace
//! ```
//!
//! These tests are `#[ignore]` and skip when `RUN_BINANCE_DEMO_INTEGRATION` is unset or demo keys
//! are missing — default workspace runs stay offline.
//!
//! ## Demo integration (opt-in)
//!
//! 1. `cp .env.example .env`
//! 2. Set `DEMO_BINANCE_KEY`, `DEMO_BINANCE_SECRET`, and `RUN_BINANCE_DEMO_INTEGRATION=1`
//! 3. Optional: `TROLLY_DEMO_SYMBOL=BTCUSDT`
//! 4. ```text
//!    cargo test --test binance_demo -- --ignored
//!    ```
//!
//! ## Follow-on checklist (after WP-014 / WP-015; not blocking WP-017)
//!
//! - [ ] Spot: demo `POST /api/v3/order` → reconcile `executionReport` on user-data stream
//! - [ ] USDM: demo `POST /fapi/v1/order` → reconcile `ORDER_TRADE_UPDATE` on listenKey stream

use std::time::Duration;

use binance_spot_exec::{
    parse_user_data_message, ApiCredentials, BinanceSpotRest, BinanceSpotUserStream, SpotUserEvent,
};
use binance_usdm_exec::{
    parse_user_events, BinanceUsdmRest, UsdmUserDataStream, UsdmExecUpdate,
};
use futures_util::{SinkExt, StreamExt};
use tokio::time::timeout;
use tokio_tungstenite::{connect_async, tungstenite::Message as WsMessage};
use trolly_stream::Message;

const USER_EVENT_WAIT: Duration = Duration::from_secs(12);

fn load_demo_env() -> bool {
    dotenvy::dotenv().ok();
    std::env::var("RUN_BINANCE_DEMO_INTEGRATION")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false)
}

fn demo_credentials() -> Option<ApiCredentials> {
    let api_key = std::env::var("DEMO_BINANCE_KEY")
        .ok()
        .filter(|v| !v.trim().is_empty())?;
    let secret_key = std::env::var("DEMO_BINANCE_SECRET")
        .ok()
        .filter(|v| !v.trim().is_empty())?;
    Some(ApiCredentials {
        api_key,
        secret_key,
    })
}

fn demo_symbol() -> String {
    std::env::var("TROLLY_DEMO_SYMBOL").unwrap_or_else(|_| "BTCUSDT".into())
}

#[derive(Debug, serde::Deserialize)]
struct DepthSnapshot {
    #[serde(rename = "lastUpdateId")]
    last_update_id: u64,
    bids: Vec<[String; 2]>,
    asks: Vec<[String; 2]>,
}

async fn fetch_demo_depth(url: &str) -> Result<DepthSnapshot, String> {
    let response = reqwest::get(url)
        .await
        .map_err(|e| format!("REST GET failed: {e}"))?;
    if let Err(e) = response.error_for_status_ref() {
        return Err(format!("REST status {}: {e}", response.status()));
    }
    response
        .json()
        .await
        .map_err(|e| format!("REST JSON decode failed: {e}"))
}

fn assert_depth_book(book: &DepthSnapshot, venue: &str, symbol: &str) {
    assert!(
        book.last_update_id > 0,
        "{venue} demo depth for {symbol} should have lastUpdateId > 0"
    );
    assert!(
        !book.bids.is_empty() && !book.asks.is_empty(),
        "{venue} demo depth for {symbol} should have bids and asks"
    );
}

async fn recv_text_until_deadline(
    read: &mut futures_util::stream::SplitStream<
        tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
    >,
    deadline: Duration,
) -> Vec<String> {
    let mut out = Vec::new();
    let deadline = tokio::time::Instant::now() + deadline;
    while tokio::time::Instant::now() < deadline {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            break;
        }
        match timeout(remaining, read.next()).await {
            Ok(Some(Ok(WsMessage::Text(text)))) => out.push(text.to_string()),
            Ok(Some(Ok(WsMessage::Ping(_)) | Ok(WsMessage::Pong(_)))) => {}
            Ok(Some(Ok(WsMessage::Close(_)))) => break,
            Ok(Some(Ok(_))) => {}
            Ok(Some(Err(_))) | Ok(None) | Err(_) => break,
        }
    }
    out
}

/// Spot demo REST depth + signed user-data subscribe on demo WS API.
#[tokio::test]
#[ignore = "live Binance spot demo; set RUN_BINANCE_DEMO_INTEGRATION=1 and demo keys in .env"]
async fn spot_demo_rest_depth_and_user_stream() {
    if !load_demo_env() {
        eprintln!("skip: RUN_BINANCE_DEMO_INTEGRATION not enabled");
        return;
    }
    let Some(credentials) = demo_credentials() else {
        eprintln!("skip: DEMO_BINANCE_KEY / DEMO_BINANCE_SECRET not set");
        return;
    };

    let symbol = demo_symbol();
    let depth_url = BinanceSpotRest::demo_depth_url(&symbol, 500);
    let book = match fetch_demo_depth(&depth_url).await {
        Ok(book) => book,
        Err(reason) => {
            eprintln!("skip: spot demo REST unavailable ({reason})");
            return;
        }
    };
    assert_depth_book(&book, "spot", &symbol);

    let stream = BinanceSpotUserStream::new(credentials);
    let subscribe = stream.subscribe_request_json();
    let ws_url = BinanceSpotUserStream::DEMO_WS_API_URL;
    let (ws, _) = match connect_async(ws_url).await {
        Ok(pair) => pair,
        Err(e) => {
            eprintln!("skip: spot demo WS connect failed ({e})");
            return;
        }
    };

    let (mut write, mut read) = ws.split();
    if write
        .send(WsMessage::Text(subscribe.into()))
        .await
        .is_err()
    {
        eprintln!("skip: spot demo WS subscribe send failed");
        return;
    }

    let frames = recv_text_until_deadline(&mut read, USER_EVENT_WAIT).await;

    let mut parsed_events = Vec::new();
    for frame in &frames {
        match parse_user_data_message(Message::Text(frame.clone().into())) {
            Ok(Some(event)) => parsed_events.push(event),
            Ok(None) => {}
            Err(e) => panic!("spot demo user-data parse error: {e}\nframe: {frame}"),
        }
    }

    if parsed_events.is_empty() {
        eprintln!(
            "skip: spot demo account idle (no user-data events within {:?}); \
             subscription ack only — place demo activity or retry after trading",
            USER_EVENT_WAIT
        );
        return;
    }

    assert!(
        parsed_events
            .iter()
            .any(|event| matches!(
                event,
                SpotUserEvent::ExecutionReport(_)
                    | SpotUserEvent::OutboundAccountPosition(_)
                    | SpotUserEvent::BalanceUpdate(_)
                    | SpotUserEvent::ListStatus(_)
            )),
        "expected at least one spot account/execution event, got: {parsed_events:?}"
    );
}

async fn demo_listen_key_request(
    method: reqwest::Method,
    api_key: &str,
) -> Result<(), String> {
    let label = method.to_string();
    let client = reqwest::Client::new();
    let response = client
        .request(method, BinanceUsdmRest::demo_listen_key_url())
        .header("X-MBX-APIKEY", api_key)
        .send()
        .await
        .map_err(|e| format!("listenKey {label} failed: {e}"))?;
    response
        .error_for_status()
        .map_err(|e| format!("listenKey {label} status: {e}"))?;
    Ok(())
}

async fn create_demo_listen_key(api_key: &str) -> Result<String, String> {
    let client = reqwest::Client::new();
    let response = client
        .post(BinanceUsdmRest::demo_listen_key_url())
        .header("X-MBX-APIKEY", api_key)
        .send()
        .await
        .map_err(|e| format!("listenKey POST failed: {e}"))?;
    let response = response
        .error_for_status()
        .map_err(|e| format!("listenKey POST status: {e}"))?;
    let body: serde_json::Value = response
        .json()
        .await
        .map_err(|e| format!("listenKey POST JSON: {e}"))?;
    body.get("listenKey")
        .and_then(|v| v.as_str())
        .map(str::to_owned)
        .ok_or_else(|| format!("listenKey missing in response: {body}"))
}

/// USDM demo REST depth + listenKey lifecycle on demo-fapi + user-data stream.
#[tokio::test]
#[ignore = "live Binance USDM demo; set RUN_BINANCE_DEMO_INTEGRATION=1 and demo keys in .env"]
async fn usdm_demo_rest_depth_listen_key_and_user_stream() {
    if !load_demo_env() {
        eprintln!("skip: RUN_BINANCE_DEMO_INTEGRATION not enabled");
        return;
    }
    let Some(credentials) = demo_credentials() else {
        eprintln!("skip: DEMO_BINANCE_KEY / DEMO_BINANCE_SECRET not set");
        return;
    };

    let symbol = demo_symbol();
    let depth_url = BinanceUsdmRest::demo_depth_url(&symbol, 500);
    let book = match fetch_demo_depth(&depth_url).await {
        Ok(book) => book,
        Err(reason) => {
            eprintln!("skip: USDM demo REST unavailable ({reason})");
            return;
        }
    };
    assert_depth_book(&book, "USDM", &symbol);

    let listen_key = match create_demo_listen_key(&credentials.api_key).await {
        Ok(key) => key,
        Err(reason) => {
            eprintln!("skip: USDM demo listenKey create failed ({reason})");
            return;
        }
    };

    if let Err(reason) = demo_listen_key_request(reqwest::Method::PUT, &credentials.api_key).await {
        eprintln!("skip: USDM demo listenKey keepalive failed ({reason})");
        return;
    }

    let stream = UsdmUserDataStream::new(&listen_key);
    let ws_url = stream.demo_websocket_url();
    let (ws, _) = match connect_async(&ws_url).await {
        Ok(pair) => pair,
        Err(e) => {
            eprintln!("skip: USDM demo user-data WS connect failed ({e})");
            let _ = demo_listen_key_request(reqwest::Method::DELETE, &credentials.api_key).await;
            return;
        }
    };

    let (_write, mut read) = ws.split();
    let frames = recv_text_until_deadline(&mut read, USER_EVENT_WAIT).await;

    let _ = demo_listen_key_request(reqwest::Method::DELETE, &credentials.api_key).await;

    let mut parsed_events = Vec::new();
    for frame in &frames {
        match parse_user_events(Message::Text(frame.clone().into())) {
            Ok(events) => parsed_events.extend(events),
            Err(e) => panic!("USDM demo user-data parse error: {e}\nframe: {frame}"),
        }
    }

    if parsed_events.is_empty() {
        eprintln!(
            "skip: USDM demo account idle (no user-data events within {:?}); \
             listenKey stream connected — place demo activity or retry after trading",
            USER_EVENT_WAIT
        );
        return;
    }

    assert!(
        parsed_events.iter().any(|event| matches!(
            event,
            UsdmExecUpdate::OrderTrade(_)
                | UsdmExecUpdate::BalanceChange(_)
                | UsdmExecUpdate::PositionChange(_)
                | UsdmExecUpdate::MarginCall(_)
        )),
        "expected at least one USDM execution/account event, got: {parsed_events:?}"
    );
}
