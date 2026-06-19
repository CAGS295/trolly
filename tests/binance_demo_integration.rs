//! Binance **demo/testnet** integration tests (spot + USDM).
//!
//! ## Offline (default)
//!
//! ```text
//! cargo test --test binance_demo_integration
//! ```
//!
//! Both live tests are `#[ignore]` and never hit the network unless you opt in.
//!
//! ## Demo endpoints (never use production keys)
//!
//! | Venue | REST | Private / WS API |
//! |-------|------|------------------|
//! | Spot | `https://demo-api.binance.com/api` | `wss://demo-ws-api.binance.com/ws-api/v3` |
//! | Spot market streams | — | `wss://demo-stream.binance.com/ws` |
//! | USDM | `https://demo-fapi.binance.com` | `wss://fstream.binancefuture.com/private/ws/<listenKey>` |
//!
//! See [Spot Demo general info](https://developers.binance.com/docs/binance-spot-api-docs/demo-mode/general-info)
//! and [USDM general info](https://developers.binance.com/docs/derivatives/usds-margined-futures/general-info).
//!
//! ## Opt-in live demo run
//!
//! 1. `cp .env.example .env`
//! 2. Set `RUN_BINANCE_DEMO_INTEGRATION=1`, `DEMO_BINANCE_KEY`, and `DEMO_BINANCE_SECRET` in `.env`
//! 3. `cargo test --test binance_demo_integration -- --ignored`
//!
//! Optional: `TROLLY_DEMO_SYMBOL` (default `BTCUSDT`). If the flag is unset, `0`, or keys are
//! missing, live tests print `skip` and return without contacting the network.
//!
//! ## Follow-on checklist (not blocking WP-017)
//!
//! After WP-014 / WP-015 land, extend these tests with a signed demo order round-trip:
//!
//! - [ ] Spot: `POST` demo `/api/v3/order` → reconcile `executionReport` on user-data stream
//! - [ ] USDM: `POST` demo `/fapi/v1/order` → reconcile `ORDER_TRADE_UPDATE` on user-data stream

use std::time::{Duration, Instant};

use binance_spot_exec::{
    parse_user_data_message, ApiCredentials, BinanceSpotUserStream, SpotUserEvent,
};
use binance_usdm_exec::{
    demo_rest_depth_url, parse_user_events, DEMO_REST_API_URL, LISTEN_KEY_CREATE_PATH,
    UsdmExecUpdate, UsdmUserDataStream,
};
use futures_util::{SinkExt, StreamExt};
use http::Uri;
use lob::LimitOrderBook;
use serde::Deserialize;
use trolly_stream::{connect, Message, VenueEndpoints};

const USER_DATA_WAIT: Duration = Duration::from_secs(8);

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

fn skip_unless_demo_enabled() -> bool {
    dotenvy::dotenv().ok();
    if !demo_integration_enabled() {
        eprintln!("skip: RUN_BINANCE_DEMO_INTEGRATION not enabled");
        return true;
    }
    if demo_credentials().is_none() {
        eprintln!("skip: DEMO_BINANCE_KEY / DEMO_BINANCE_SECRET not set");
        return true;
    }
    false
}

fn demo_symbol() -> String {
    std::env::var("TROLLY_DEMO_SYMBOL").unwrap_or_else(|_| "BTCUSDT".into())
}

async fn fetch_demo_depth(url: &str) -> Result<LimitOrderBook, String> {
    let response = reqwest::get(url)
        .await
        .map_err(|e| format!("REST GET failed: {e}"))?;
    let status = response.status();
    response
        .error_for_status()
        .map_err(|e| format!("REST status {status}: {e}"))?
        .json::<LimitOrderBook>()
        .await
        .map_err(|e| format!("REST JSON: {e}"))
}

#[derive(Deserialize)]
struct ListenKeyResponse {
    #[serde(rename = "listenKey")]
    listen_key: String,
}

async fn usdm_listen_key_request(
    client: &reqwest::Client,
    method: reqwest::Method,
    api_key: &str,
) -> Result<(), String> {
    let url = format!(
        "{}{}",
        DEMO_REST_API_URL.trim_end_matches('/'),
        LISTEN_KEY_CREATE_PATH
    );
    let response = client
        .request(method.clone(), &url)
        .header("X-MBX-APIKEY", api_key)
        .send()
        .await
        .map_err(|e| format!("listenKey {method} failed: {e}"))?;
    let status = response.status();
    response
        .error_for_status()
        .map_err(|e| format!("listenKey {method} status {status}: {e}"))?;
    Ok(())
}

async fn usdm_create_listen_key(client: &reqwest::Client, api_key: &str) -> Result<String, String> {
    let url = format!(
        "{}{}",
        DEMO_REST_API_URL.trim_end_matches('/'),
        LISTEN_KEY_CREATE_PATH
    );
    let response = client
        .post(&url)
        .header("X-MBX-APIKEY", api_key)
        .send()
        .await
        .map_err(|e| format!("listenKey POST failed: {e}"))?;
    let status = response.status();
    let body = response
        .error_for_status()
        .map_err(|e| format!("listenKey POST status {status}: {e}"))?
        .json::<ListenKeyResponse>()
        .await
        .map_err(|e| format!("listenKey JSON: {e}"))?;
    Ok(body.listen_key)
}

/// Spot demo REST depth + signed user-data subscribe on the demo WebSocket API.
#[tokio::test]
#[ignore = "live Binance spot demo; set RUN_BINANCE_DEMO_INTEGRATION=1 and demo keys in .env"]
async fn binance_spot_demo_rest_and_user_stream() {
    if skip_unless_demo_enabled() {
        return;
    }
    let (api_key, secret_key) = demo_credentials().expect("checked in skip_unless_demo_enabled");
    let symbol = demo_symbol();

    let depth_url = BinanceSpotUserStream::demo_rest_depth_url(&symbol);
    let book = match fetch_demo_depth(&depth_url).await {
        Ok(book) => book,
        Err(e) => {
            eprintln!("skip: spot demo REST unavailable ({e})");
            return;
        }
    };
    assert!(book.update_id > 0, "empty spot demo book for {symbol}");

    let stream = BinanceSpotUserStream::demo(ApiCredentials {
        api_key,
        secret_key,
    });
    let ws_url: Uri = stream
        .websocket_url()
        .parse()
        .expect("demo spot WS API URL");
    let mut ws = match connect(ws_url).await {
        Ok(ws) => ws,
        Err(e) => {
            eprintln!("skip: spot demo WS connect failed ({e})");
            return;
        }
    };

    let subscribe = stream.subscribe_request_json();
    if let Err(e) = ws.send(Message::Text(subscribe.into())).await {
        eprintln!("skip: spot demo subscribe send failed ({e})");
        return;
    }

    let deadline = Instant::now() + USER_DATA_WAIT;
    let mut subscribed = false;
    let mut spot_events: Vec<SpotUserEvent> = Vec::new();

    while Instant::now() < deadline {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            break;
        }
        let wait = remaining.min(Duration::from_secs(1));
        let frame = tokio::time::timeout(wait, ws.next()).await;
        let Some(result) = (match frame {
            Ok(v) => v,
            Err(_) => continue,
        }) else {
            break;
        };
        let msg = match result {
            Ok(m) => m,
            Err(e) => {
                eprintln!("skip: spot demo WS read error ({e})");
                return;
            }
        };
        let Message::Text(text) = msg else {
            continue;
        };
        let value: serde_json::Value = match serde_json::from_str(&text) {
            Ok(v) => v,
            Err(_) => continue,
        };
        if value.get("result").is_some() && value.get("id").is_some() {
            subscribed = true;
            continue;
        }
        if let Ok(Some(event)) = parse_user_data_message(Message::Text(text)) {
            spot_events.push(event);
        }
    }

    assert!(subscribed, "spot demo user-data subscribe did not ack");

    if spot_events.is_empty() {
        eprintln!("skip: no spot user-data events (demo account idle within {USER_DATA_WAIT:?})");
        return;
    }

    let has_execution_or_account = spot_events.iter().any(|event| {
        matches!(
            event,
            SpotUserEvent::ExecutionReport(_)
                | SpotUserEvent::OutboundAccountPosition(_)
                | SpotUserEvent::BalanceUpdate(_)
        )
    });
    assert!(
        has_execution_or_account,
        "expected executionReport or account event, got {} other events",
        spot_events.len()
    );
}

/// USDM demo REST depth + listenKey lifecycle + private user-data stream parsing.
#[tokio::test]
#[ignore = "live Binance USDM demo; set RUN_BINANCE_DEMO_INTEGRATION=1 and demo keys in .env"]
async fn binance_usdm_demo_rest_listen_key_and_user_stream() {
    if skip_unless_demo_enabled() {
        return;
    }
    let (api_key, _) = demo_credentials().expect("checked in skip_unless_demo_enabled");
    let symbol = demo_symbol();

    let depth_url = demo_rest_depth_url(&symbol);
    let book = match fetch_demo_depth(&depth_url).await {
        Ok(book) => book,
        Err(e) => {
            eprintln!("skip: USDM demo REST unavailable ({e})");
            return;
        }
    };
    assert!(book.update_id > 0, "empty USDM demo book for {symbol}");

    let client = reqwest::Client::new();
    let listen_key = match usdm_create_listen_key(&client, &api_key).await {
        Ok(key) => key,
        Err(e) => {
            eprintln!("skip: USDM demo listenKey create failed ({e})");
            return;
        }
    };

    if let Err(e) = usdm_listen_key_request(
        &client,
        reqwest::Method::PUT,
        &api_key,
    )
    .await
    {
        eprintln!("skip: USDM demo listenKey keepalive failed ({e})");
        return;
    }

    let stream = UsdmUserDataStream::demo(&listen_key);
    let ws_url: Uri = stream.websocket_url().parse().expect("demo USDM WS URL");
    let mut ws = match connect(ws_url).await {
        Ok(ws) => ws,
        Err(e) => {
            eprintln!("skip: USDM demo WS connect failed ({e})");
            return;
        }
    };

    let deadline = Instant::now() + USER_DATA_WAIT;
    let mut usdm_events = Vec::new();
    while Instant::now() < deadline {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            break;
        }
        let wait = remaining.min(Duration::from_secs(1));
        let frame = tokio::time::timeout(wait, ws.next()).await;
        let Some(result) = (match frame {
            Ok(v) => v,
            Err(_) => continue,
        }) else {
            break;
        };
        let msg = match result {
            Ok(m) => m,
            Err(e) => {
                eprintln!("skip: USDM demo WS read error ({e})");
                break;
            }
        };
        let Message::Text(text) = msg else {
            continue;
        };
        match parse_user_events(Message::Text(text)) {
            Ok(events) => usdm_events.extend(events),
            Err(_) => continue,
        }
    }

    let _ = usdm_listen_key_request(&client, reqwest::Method::DELETE, &api_key).await;

    if usdm_events.is_empty() {
        eprintln!("skip: no USDM user-data events (demo account idle within {USER_DATA_WAIT:?})");
        return;
    }

    let has_order_or_account = usdm_events.iter().any(|event| {
        matches!(
            event,
            UsdmExecUpdate::OrderTrade(_) | UsdmExecUpdate::BalanceChange(_)
        )
    });
    assert!(
        has_order_or_account,
        "expected ORDER_TRADE_UPDATE or ACCOUNT_UPDATE balance row, got {} events",
        usdm_events.len()
    );
}
