//! Binance demo / testnet integration tests — spot and USDM.
//!
//! These tests connect to Binance **demo trading** endpoints (simulated funds,
//! real credentials):
//!
//! | Venue | Service          | URL                                                         |
//! |-------|------------------|-------------------------------------------------------------|
//! | Spot  | REST depth       | `https://demo-api.binance.com/api/v3/depth`                 |
//! | Spot  | WS API (signed)  | `wss://demo-ws-api.binance.com:443/ws-api/v3`               |
//! | Spot  | WS market stream | `wss://demo-stream.binance.com:9443/ws`                     |
//! | USDM  | REST depth       | `https://demo-fapi.binance.com/fapi/v1/depth`               |
//! | USDM  | listenKey REST   | `https://demo-fapi.binance.com/fapi/v1/listenKey`           |
//! | USDM  | WS user-data     | `wss://fstream.binance.com/ws/<listenKey>` (demo listenKey) |
//!
//! ## Setup
//!
//! ```text
//! cp .env.example .env
//! ```
//!
//! Then in `.env`:
//!
//! ```env
//! RUN_BINANCE_DEMO_INTEGRATION=1
//! TROLLY_DEMO_SYMBOL=BTCUSDT          # optional, defaults to BTCUSDT
//! DEMO_BINANCE_KEY=<your-api-key>     # required for signed/listenKey tests
//! DEMO_BINANCE_SECRET=<your-secret>   # required for spot WS API subscribe
//! ```
//!
//! Run the ignored tests:
//!
//! ```text
//! cargo test --test binance_demo -- --ignored
//! ```
//!
//! ## Offline default
//!
//! `cargo test --workspace` never hits the network: every test here carries
//! `#[ignore]`.  Tests also skip cleanly when `RUN_BINANCE_DEMO_INTEGRATION`
//! is unset or `0`, so CI never breaks.
//!
//! ## Optional follow-on (requires WP-014/WP-015 order placement)
//!
//! Demo order place round-trip — wire up after exec order clients are live:
//!
//! - [ ] `POST /api/v3/order` (spot demo) — LIMIT far OTM, immediately cancel
//! - [ ] `POST /fapi/v1/order` (USDM demo) — LIMIT far OTM, immediately cancel
//! - [ ] Assert `executionReport` / `ORDER_TRADE_UPDATE` arrives on WS within timeout
//!
//! These are left as a sub-checklist; they depend on signed REST order clients in
//! `binance-spot-exec` and `binance-usdm-exec` (WP-014/WP-015).

use std::time::Duration;

use binance_spot_exec::{
    spot_demo_endpoints as spot_demo, ApiCredentials, BinanceSpotUserStream,
};
use binance_usdm_exec::{parse_user_events, usdm_demo_endpoints as usdm_demo, UsdmExecUpdate};
use futures_util::{SinkExt, StreamExt};
use serde_json::Value;
use trolly::Message;

// ── helpers ──────────────────────────────────────────────────────────────────

fn demo_enabled() -> bool {
    dotenvy::dotenv().ok();
    std::env::var("RUN_BINANCE_DEMO_INTEGRATION")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false)
}

fn demo_symbol() -> String {
    std::env::var("TROLLY_DEMO_SYMBOL").unwrap_or_else(|_| "BTCUSDT".into())
}

fn demo_credentials() -> Option<ApiCredentials> {
    let key = std::env::var("DEMO_BINANCE_KEY")
        .ok()
        .filter(|v| !v.is_empty())?;
    let secret = std::env::var("DEMO_BINANCE_SECRET")
        .ok()
        .filter(|v| !v.is_empty())?;
    Some(ApiCredentials {
        api_key: key,
        secret_key: secret,
    })
}

fn demo_api_key() -> Option<String> {
    std::env::var("DEMO_BINANCE_KEY")
        .ok()
        .filter(|v| !v.is_empty())
}

// ── spot ─────────────────────────────────────────────────────────────────────

/// Fetches a public depth snapshot from the Binance spot demo REST endpoint.
///
/// No credentials required; validates response structure (bids + asks present).
#[tokio::test]
#[ignore = "live demo REST; set RUN_BINANCE_DEMO_INTEGRATION=1 in .env"]
async fn demo_spot_rest_depth_snapshot() {
    if !demo_enabled() {
        eprintln!("skip: RUN_BINANCE_DEMO_INTEGRATION not enabled");
        return;
    }

    let symbol = demo_symbol();
    let url = format!(
        "{}/api/v3/depth?symbol={}&limit=5",
        spot_demo::REST_BASE_URL,
        symbol.to_uppercase()
    );

    let resp = reqwest::get(&url)
        .await
        .unwrap_or_else(|e| panic!("spot demo REST GET failed: {e}"));

    assert!(
        resp.status().is_success(),
        "spot demo REST returned non-2xx: {}",
        resp.status()
    );

    let body: Value = resp
        .json()
        .await
        .expect("spot demo depth response is not valid JSON");

    assert!(
        body.get("bids")
            .and_then(Value::as_array)
            .map(|a| !a.is_empty())
            .unwrap_or(false),
        "expected non-empty bids in spot demo depth response: {body}"
    );
    assert!(
        body.get("asks")
            .and_then(Value::as_array)
            .map(|a| !a.is_empty())
            .unwrap_or(false),
        "expected non-empty asks in spot demo depth response: {body}"
    );

    eprintln!(
        "spot demo REST depth OK  symbol={symbol}  lastUpdateId={}",
        body.get("lastUpdateId").and_then(Value::as_u64).unwrap_or(0)
    );
}

/// Connects to the Binance spot demo WS API and sends a signed user-data subscribe.
///
/// Requires `DEMO_BINANCE_KEY` and `DEMO_BINANCE_SECRET` in `.env`.
/// Skips with a clear message when credentials are absent.
///
/// On success the server returns an ack (`"result": null`).  If the demo account
/// has recent activity an `executionReport` / `outboundAccountPosition` event
/// may arrive; the test validates it through the spot exec parser.  A silent
/// (idle) account is not a failure — the test prints a skip message and returns.
#[tokio::test]
#[ignore = "live demo WS API; set RUN_BINANCE_DEMO_INTEGRATION=1 and DEMO_BINANCE_KEY/SECRET in .env"]
async fn demo_spot_ws_user_data_subscribe() {
    if !demo_enabled() {
        eprintln!("skip: RUN_BINANCE_DEMO_INTEGRATION not enabled");
        return;
    }

    let Some(creds) = demo_credentials() else {
        eprintln!("skip: DEMO_BINANCE_KEY / DEMO_BINANCE_SECRET not set");
        return;
    };

    let stream = BinanceSpotUserStream::new(creds);
    let subscribe_payload = stream.subscribe_request_json();

    let (mut ws, _) = tokio_tungstenite::connect_async(spot_demo::WS_API_URL)
        .await
        .unwrap_or_else(|e| panic!("spot demo WS API connect failed: {e}"));

    ws.send(Message::Text(subscribe_payload.into()))
        .await
        .expect("send spot demo WS subscribe");

    // Wait for the subscribe acknowledgement.
    let raw = tokio::time::timeout(Duration::from_secs(15), ws.next())
        .await
        .expect("timed out waiting for spot WS API ack (15 s)")
        .expect("WS stream closed before ack")
        .expect("WS receive error on ack");

    let text = match raw {
        Message::Text(t) => t.to_string(),
        Message::Binary(b) => String::from_utf8_lossy(&b).into_owned(),
        Message::Ping(_) | Message::Pong(_) | Message::Frame(_) => {
            eprintln!("skip: received control frame instead of subscribe ack");
            let _ = ws.close(None).await;
            return;
        }
        Message::Close(_) => {
            panic!("spot demo WS closed before ack");
        }
    };

    let response: Value = serde_json::from_str(&text)
        .unwrap_or_else(|_| panic!("spot WS API ack is not valid JSON: {text}"));

    // A non-2xx `status` in the WS API response means an API-level error.
    if let Some(status) = response.get("status").and_then(Value::as_i64) {
        assert!(
            (200..300).contains(&status),
            "spot demo WS API subscribe returned error status {status}: {response}"
        );
    }

    // Successful subscribe ack: `"result": null`.
    if let Some(result) = response.get("result") {
        assert!(
            result.is_null(),
            "expected null result on subscribe ack, got: {response}"
        );
    }

    eprintln!("spot demo WS subscribe ack OK: {response}");

    // Optionally receive one execution event if the demo account has recent activity.
    match tokio::time::timeout(Duration::from_secs(5), ws.next()).await {
        Ok(Some(Ok(Message::Text(raw_event)))) => {
            let event_text = raw_event.to_string();
            let event_val: Value =
                serde_json::from_str(&event_text).unwrap_or(Value::Null);
            let event_type = event_val
                .get("e")
                .and_then(Value::as_str)
                .unwrap_or("unknown");

            eprintln!(
                "spot demo WS event type={event_type}: {}",
                &event_text[..event_text.len().min(200)]
            );

            // For known execution events, validate they parse correctly.
            if matches!(
                event_type,
                "executionReport" | "outboundAccountPosition" | "balanceUpdate"
            ) {
                let parse_result = binance_spot_exec::parse_user_data_message(
                    Message::Text(event_text.into()),
                );
                assert!(
                    parse_result.is_ok(),
                    "spot exec parser rejected live {event_type} event: {parse_result:?}"
                );
                eprintln!("spot demo exec parse OK for event_type={event_type}");
            }
        }
        _ => {
            eprintln!(
                "skip event check: no spot demo execution events within 5 s (demo account idle)"
            );
        }
    }

    let _ = ws.close(None).await;
}

// ── USDM ─────────────────────────────────────────────────────────────────────

/// Fetches a public depth snapshot from the Binance USDM demo futures REST endpoint.
///
/// No credentials required; validates response structure (bids + asks present).
#[tokio::test]
#[ignore = "live demo REST; set RUN_BINANCE_DEMO_INTEGRATION=1 in .env"]
async fn demo_usdm_rest_depth_snapshot() {
    if !demo_enabled() {
        eprintln!("skip: RUN_BINANCE_DEMO_INTEGRATION not enabled");
        return;
    }

    let symbol = demo_symbol();
    let url = format!(
        "{}/fapi/v1/depth?symbol={}&limit=5",
        usdm_demo::REST_BASE_URL,
        symbol.to_uppercase()
    );

    let resp = reqwest::get(&url)
        .await
        .unwrap_or_else(|e| panic!("USDM demo REST GET failed: {e}"));

    assert!(
        resp.status().is_success(),
        "USDM demo REST returned non-2xx: {}",
        resp.status()
    );

    let body: Value = resp
        .json()
        .await
        .expect("USDM demo depth response is not valid JSON");

    assert!(
        body.get("bids")
            .and_then(Value::as_array)
            .map(|a| !a.is_empty())
            .unwrap_or(false),
        "expected non-empty bids in USDM demo depth response: {body}"
    );
    assert!(
        body.get("asks")
            .and_then(Value::as_array)
            .map(|a| !a.is_empty())
            .unwrap_or(false),
        "expected non-empty asks in USDM demo depth response: {body}"
    );

    eprintln!(
        "USDM demo REST depth OK  symbol={symbol}  lastUpdateId={}",
        body.get("lastUpdateId").and_then(Value::as_u64).unwrap_or(0)
    );
}

/// Creates, renews (keep-alive), and deletes a USDM demo `listenKey` via
/// `demo-fapi.binance.com`.
///
/// Requires `DEMO_BINANCE_KEY` in `.env`.
/// Skips with a clear message when the key is absent.
#[tokio::test]
#[ignore = "live demo REST; set RUN_BINANCE_DEMO_INTEGRATION=1 and DEMO_BINANCE_KEY in .env"]
async fn demo_usdm_listen_key_lifecycle() {
    if !demo_enabled() {
        eprintln!("skip: RUN_BINANCE_DEMO_INTEGRATION not enabled");
        return;
    }

    let Some(api_key) = demo_api_key() else {
        eprintln!("skip: DEMO_BINANCE_KEY not set");
        return;
    };

    let client = reqwest::Client::new();
    let listen_key_url = format!("{}/fapi/v1/listenKey", usdm_demo::REST_BASE_URL);

    // POST — create listenKey.
    let create_resp = client
        .post(&listen_key_url)
        .header("X-MBX-APIKEY", &api_key)
        .send()
        .await
        .unwrap_or_else(|e| panic!("POST listenKey failed: {e}"));

    assert!(
        create_resp.status().is_success(),
        "POST listenKey non-2xx: {}",
        create_resp.status()
    );

    let create_body: Value = create_resp
        .json()
        .await
        .expect("POST listenKey response is not valid JSON");

    let listen_key = create_body
        .get("listenKey")
        .and_then(Value::as_str)
        .expect("listenKey absent in POST /fapi/v1/listenKey response")
        .to_owned();

    assert!(!listen_key.is_empty(), "listenKey must be non-empty");
    eprintln!(
        "USDM demo listenKey created: {}…",
        &listen_key[..listen_key.len().min(16)]
    );

    // PUT — keep-alive.
    let keepalive_resp = client
        .put(&listen_key_url)
        .header("X-MBX-APIKEY", &api_key)
        .query(&[("listenKey", &listen_key)])
        .send()
        .await
        .unwrap_or_else(|e| panic!("PUT listenKey keep-alive failed: {e}"));

    assert!(
        keepalive_resp.status().is_success(),
        "PUT listenKey non-2xx: {}",
        keepalive_resp.status()
    );
    eprintln!("USDM demo listenKey keep-alive OK");

    // DELETE — clean up.
    let delete_resp = client
        .delete(&listen_key_url)
        .header("X-MBX-APIKEY", &api_key)
        .query(&[("listenKey", &listen_key)])
        .send()
        .await
        .unwrap_or_else(|e| panic!("DELETE listenKey failed: {e}"));

    assert!(
        delete_resp.status().is_success(),
        "DELETE listenKey non-2xx: {}",
        delete_resp.status()
    );
    eprintln!("USDM demo listenKey deleted OK");
}

/// Opens a USDM demo user-data WebSocket stream and waits for execution events.
///
/// Requires `DEMO_BINANCE_KEY` in `.env` to obtain a `listenKey` from
/// `demo-fapi.binance.com`.  The stream URL is
/// `wss://fstream.binance.com/ws/<listenKey>`.
///
/// When the demo account is idle no events arrive within the 10 s window; the
/// test skips the event-parse assertion and returns successfully.  When events do
/// arrive, `ORDER_TRADE_UPDATE` and `ACCOUNT_UPDATE` payloads are validated
/// through the USDM exec parser.  The `listenKey` is deleted on exit.
#[tokio::test]
#[ignore = "live demo WS; set RUN_BINANCE_DEMO_INTEGRATION=1 and DEMO_BINANCE_KEY in .env"]
async fn demo_usdm_user_data_stream() {
    if !demo_enabled() {
        eprintln!("skip: RUN_BINANCE_DEMO_INTEGRATION not enabled");
        return;
    }

    let Some(api_key) = demo_api_key() else {
        eprintln!("skip: DEMO_BINANCE_KEY not set");
        return;
    };

    let client = reqwest::Client::new();
    let listen_key_url = format!("{}/fapi/v1/listenKey", usdm_demo::REST_BASE_URL);

    // Create listenKey from demo-fapi.
    let create_resp = client
        .post(&listen_key_url)
        .header("X-MBX-APIKEY", &api_key)
        .send()
        .await
        .unwrap_or_else(|e| panic!("POST listenKey for WS test failed: {e}"));

    assert!(
        create_resp.status().is_success(),
        "POST listenKey non-2xx: {}",
        create_resp.status()
    );

    let create_body: Value = create_resp
        .json()
        .await
        .expect("POST listenKey JSON decode failed");

    let listen_key = create_body
        .get("listenKey")
        .and_then(Value::as_str)
        .expect("listenKey absent in POST response")
        .to_owned();

    // Connect to the USDM user-data stream using the listenKey.
    let ws_url = format!("{}/ws/{}", usdm_demo::STREAM_BASE_URL, listen_key);

    let (mut ws, _) = tokio_tungstenite::connect_async(&ws_url)
        .await
        .unwrap_or_else(|e| panic!("USDM demo WS connect failed: {e}"));

    eprintln!("USDM demo WS connected (stream: {ws_url})");

    // Wait for any execution event — idle accounts produce no events.
    match tokio::time::timeout(Duration::from_secs(10), ws.next()).await {
        Ok(Some(Ok(Message::Text(raw_event)))) => {
            let event_text = raw_event.to_string();
            let event_val: Value =
                serde_json::from_str(&event_text).unwrap_or(Value::Null);
            let event_type = event_val
                .get("e")
                .and_then(Value::as_str)
                .unwrap_or("unknown");

            eprintln!(
                "USDM demo WS event type={event_type}: {}",
                &event_text[..event_text.len().min(200)]
            );

            let events = parse_user_events(Message::Text(event_text.clone().into()));

            assert!(
                events.is_ok(),
                "USDM exec parser rejected live event: {:?}\nraw: {event_text}",
                events.unwrap_err()
            );

            for ev in events.unwrap() {
                match ev {
                    UsdmExecUpdate::OrderTrade(o) => {
                        eprintln!(
                            "  ORDER_TRADE_UPDATE  symbol={}  side={}  status={}",
                            o.symbol, o.side, o.order_status
                        );
                    }
                    UsdmExecUpdate::BalanceChange(b) => {
                        eprintln!("  ACCOUNT_UPDATE BalanceChange  asset={}", b.asset);
                    }
                    UsdmExecUpdate::PositionChange(p) => {
                        eprintln!("  ACCOUNT_UPDATE PositionChange  symbol={}", p.symbol);
                    }
                    UsdmExecUpdate::MarginCall(m) => {
                        eprintln!("  MARGIN_CALL  event_time={}", m.event_time);
                    }
                    UsdmExecUpdate::ListenKeyExpired => {
                        eprintln!("  listenKeyExpired (demo key already invalidated)");
                    }
                }
            }
        }
        Ok(Some(Ok(Message::Ping(_)))) => {
            eprintln!("skip event check: received Ping frame (stream healthy but idle)");
        }
        Ok(Some(Ok(_))) => {
            eprintln!("skip event check: received non-text WS frame");
        }
        Ok(Some(Err(e))) => {
            eprintln!("skip event check: WS error ({e})");
        }
        Ok(None) | Err(_) => {
            eprintln!(
                "skip event check: no USDM demo events within 10 s (demo account idle)"
            );
        }
    }

    let _ = ws.close(None).await;

    // Always clean up the listenKey.
    let _ = client
        .delete(&listen_key_url)
        .header("X-MBX-APIKEY", &api_key)
        .query(&[("listenKey", &listen_key)])
        .send()
        .await;

    eprintln!("USDM demo listenKey cleaned up");
}
