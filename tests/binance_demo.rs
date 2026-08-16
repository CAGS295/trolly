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
//! ## Demo order reconcile (extra opt-in)
//!
//! The ignored `*_market_order_reconciles_*` tests also require
//! `RUN_BINANCE_DEMO_ORDERS=1` because they place tiny market orders against demo
//! balances. They route user-data frames through the execution crates' existing
//! multiplexor/bookkeeping paths and never use production hosts.

use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use binance_spot_exec::{
    build_multiplexor as build_spot_multiplexor, ingest_user_data as ingest_spot_user_data,
    parse_user_data_message, spot_depth_rest_url, AccountBook, ApiCredentials,
    BinanceSpotUserStream, ExecutionReport, NativeTlsTransport as SpotNativeTlsTransport,
    OrderSide as SpotOrderSide, PlaceOrderRequest as SpotPlaceOrderRequest, SpotExecContext,
    SpotExecHandler, SpotOrderClient, SpotUserEvent, SPOT_DEMO_ORDER_BASE_URL,
    SPOT_DEMO_REST_BASE_URL,
};
use binance_usdm_exec::{
    build_multiplexor_with_context as build_usdm_multiplexor_with_context,
    ingest_user_data as ingest_usdm_user_data, parse_user_events, usdm_depth_rest_url,
    ApiCredentials as UsdmCredentials, ListenKeyClient,
    NativeTlsTransport as UsdmNativeTlsTransport, OrderSide as UsdmOrderSide, OrderTradeUpdate,
    PlaceOrderRequest as UsdmPlaceOrderRequest, PositionSide as UsdmPositionSide, UsdmExec,
    UsdmExecContext, UsdmExecHandler, UsdmExecUpdate, UsdmOrderClient, UsdmUserDataStream,
    USDM_DEMO_REST_BASE_URL,
};
use futures_util::{SinkExt, StreamExt};
use http::Uri;
use serde_json::Value;
use tokio::sync::mpsc;
use tokio::time::timeout;
use tokio_tungstenite::connect_async_tls_with_config;
use trolly_stream::{Message, MonitorMultiplexor, VenueEndpoints};

const EVENT_WAIT: Duration = Duration::from_secs(20);
const ORDER_RECONCILE_WAIT: Duration = Duration::from_secs(45);

type DemoSocket =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

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

fn skip_if_demo_orders_disabled() -> bool {
    if skip_if_demo_disabled() {
        return true;
    }
    if std::env::var("RUN_BINANCE_DEMO_ORDERS").ok().as_deref() != Some("1") {
        eprintln!("skip: set RUN_BINANCE_DEMO_ORDERS=1 to place demo orders");
        return true;
    }
    false
}

fn demo_spot_order_qty() -> String {
    std::env::var("TROLLY_DEMO_SPOT_ORDER_QTY").unwrap_or_else(|_| "0.0001".into())
}

fn demo_usdm_order_qty() -> String {
    std::env::var("TROLLY_DEMO_USDM_ORDER_QTY").unwrap_or_else(|_| "0.001".into())
}

fn demo_usdm_position_side() -> UsdmPositionSide {
    let value = std::env::var("TROLLY_DEMO_USDM_POSITION_SIDE").unwrap_or_else(|_| "BOTH".into());
    UsdmPositionSide::parse(&value)
        .unwrap_or_else(|_| panic!("invalid TROLLY_DEMO_USDM_POSITION_SIDE={value}"))
}

fn demo_client_order_id(prefix: &str) -> String {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock before unix epoch")
        .as_millis();
    format!("trolly{prefix}{millis}")
}

fn is_terminal_order_status(status: &str) -> bool {
    matches!(status, "FILLED" | "CANCELED" | "EXPIRED" | "REJECTED")
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
    assert!(status.is_success(), "REST {url} returned {status}: {body}");
    serde_json::from_str(&body).unwrap_or_else(|e| panic!("REST JSON parse failed: {e}"))
}

async fn connect_ws(url: &str) -> DemoSocket {
    let uri: Uri = url.parse().expect("valid websocket uri");
    let (socket, _) = connect_async_tls_with_config(uri, None, false, None)
        .await
        .unwrap_or_else(|e| panic!("websocket connect {url} failed: {e}"));
    socket
}

async fn wait_for_spot_subscribe_ack(socket: &mut DemoSocket) {
    let mut subscribed = false;
    timeout(EVENT_WAIT, async {
        while let Some(frame) = socket.next().await {
            let frame = frame.expect("websocket frame");
            let Message::Text(text) = Message::from(frame) else {
                continue;
            };
            let value: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
            if value.get("result").is_some() && value.get("id").is_some() {
                subscribed = true;
                break;
            }
        }
    })
    .await
    .expect("timed out waiting for spot user-data subscribe ack");
    assert!(subscribed, "demo spot user-data subscribe ack not received");
}

async fn wait_for_spot_order_reconcile(
    socket: &mut DemoSocket,
    hub: &mut MonitorMultiplexor<SpotExecHandler, ()>,
    rx: &mut mpsc::UnboundedReceiver<SpotUserEvent>,
    order_id: i64,
    client_order_id: &str,
) -> ExecutionReport {
    let mut last_matching: Option<ExecutionReport> = None;
    let result = timeout(ORDER_RECONCILE_WAIT, async {
        while let Some(frame) = socket.next().await {
            let frame = frame.expect("websocket frame");
            let Message::Text(text) = Message::from(frame) else {
                continue;
            };

            ingest_spot_user_data(hub, Message::Text(text));
            while let Ok(event) = rx.try_recv() {
                let SpotUserEvent::ExecutionReport(report) = event else {
                    continue;
                };
                if report.order_id == order_id || report.client_order_id == client_order_id {
                    last_matching = Some(report.clone());
                    if is_terminal_order_status(&report.order_status) {
                        return report;
                    }
                }
            }
        }
        panic!("spot demo websocket closed before order reconciliation");
    })
    .await;

    result.unwrap_or_else(|_| {
        panic!(
            "timed out waiting for terminal spot executionReport for order {order_id} / {client_order_id}; last matching: {last_matching:?}"
        )
    })
}

async fn wait_for_usdm_order_reconcile(
    socket: &mut DemoSocket,
    hub: &mut MonitorMultiplexor<UsdmExecHandler, UsdmExec>,
    rx: &mut mpsc::UnboundedReceiver<UsdmExecUpdate>,
    order_id: i64,
    client_order_id: &str,
) -> OrderTradeUpdate {
    let mut last_matching: Option<OrderTradeUpdate> = None;
    let result = timeout(ORDER_RECONCILE_WAIT, async {
        while let Some(frame) = socket.next().await {
            let frame = frame.expect("websocket frame");
            let Message::Text(text) = Message::from(frame) else {
                continue;
            };

            ingest_usdm_user_data(hub, Message::Text(text));
            while let Ok(event) = rx.try_recv() {
                let UsdmExecUpdate::OrderTrade(report) = event else {
                    continue;
                };
                if report.order_id == order_id || report.client_order_id == client_order_id {
                    last_matching = Some(report.clone());
                    if is_terminal_order_status(&report.order_status) {
                        return report;
                    }
                }
            }
        }
        panic!("USDM demo websocket closed before order reconciliation");
    })
    .await;

    result.unwrap_or_else(|_| {
        panic!(
            "timed out waiting for terminal USDM ORDER_TRADE_UPDATE for order {order_id} / {client_order_id}; last matching: {last_matching:?}"
        )
    })
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

/// Spot demo: place a tiny market order and reconcile the terminal executionReport through AccountBook.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "live Binance spot demo order; set RUN_BINANCE_DEMO_ORDERS=1 plus demo keys and run with --ignored"]
async fn spot_demo_market_order_reconciles_through_user_stream_bookkeeping() {
    if skip_if_demo_orders_disabled() {
        return;
    }
    let (credentials, _) = load_demo_credentials().expect("credentials checked");
    let symbol = demo_symbol();
    let client_order_id = demo_client_order_id("spot");

    let stream = BinanceSpotUserStream::demo(credentials.clone());
    assert!(stream.websocket_url().contains("demo-ws-api.binance.com"));
    let mut socket = connect_ws(&stream.websocket_url()).await;
    socket
        .send(Message::Text(stream.subscribe_request_json().into()).into())
        .await
        .expect("send subscribe");
    wait_for_spot_subscribe_ack(&mut socket).await;

    let (tx, mut rx) = mpsc::unbounded_channel();
    let account = Arc::new(Mutex::new(AccountBook::default()));
    let ctx = SpotExecContext {
        events: tx,
        account: account.clone(),
    };
    let mut hub = build_spot_multiplexor(&[symbol.as_str()], ctx);

    let client = SpotOrderClient::new(credentials, SpotNativeTlsTransport::new())
        .with_base_url(SPOT_DEMO_ORDER_BASE_URL);
    assert_eq!(client.base_url, SPOT_DEMO_ORDER_BASE_URL);

    let mut request =
        SpotPlaceOrderRequest::market(&symbol, SpotOrderSide::Buy, demo_spot_order_qty());
    request.new_client_order_id = Some(client_order_id.clone());
    let ack = client
        .place_order(request)
        .await
        .expect("place spot demo market order");
    assert_eq!(ack.symbol, symbol.to_ascii_uppercase());
    assert_eq!(ack.client_order_id, client_order_id);

    let report = wait_for_spot_order_reconcile(
        &mut socket,
        &mut hub,
        &mut rx,
        ack.order_id,
        &client_order_id,
    )
    .await;

    assert_eq!(report.symbol, symbol.to_ascii_uppercase());
    assert_eq!(report.client_order_id, client_order_id);
    assert_eq!(report.order_id, ack.order_id);
    assert!(is_terminal_order_status(&report.order_status));
    assert!(
        !account
            .lock()
            .expect("account lock poisoned")
            .open_orders
            .contains_key(&ack.order_id),
        "terminal spot order should be cleared by AccountBook"
    );
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
    let listen_key = listen_client.create().await.expect("demo listenKey create");
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

/// USDM demo: place a tiny market order and reconcile ORDER_TRADE_UPDATE through symbol bookkeeping.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "live Binance USDM demo order; set RUN_BINANCE_DEMO_ORDERS=1 plus demo keys and run with --ignored"]
async fn usdm_demo_market_order_reconciles_through_user_stream_bookkeeping() {
    if skip_if_demo_orders_disabled() {
        return;
    }
    let (_, credentials) = load_demo_credentials().expect("credentials checked");
    let symbol = demo_symbol();
    let client_order_id = demo_client_order_id("usdm");

    let listen_client = ListenKeyClient::demo(credentials.clone());
    let listen_key = listen_client.create().await.expect("demo listenKey create");
    assert!(!listen_key.is_empty(), "empty listenKey");

    let stream = UsdmUserDataStream::demo(&listen_key)
        .with_events_filter("ORDER_TRADE_UPDATE/ACCOUNT_UPDATE");
    assert!(stream.websocket_url().contains("fstream.binancefuture.com"));
    let mut socket = connect_ws(&stream.websocket_url()).await;
    listen_client
        .keepalive()
        .await
        .expect("demo listenKey keepalive");

    let (tx, mut rx) = mpsc::unbounded_channel();
    let ctx = UsdmExecContext::new(Some(tx));
    let mut hub = build_usdm_multiplexor_with_context(&[symbol.as_str()], ctx);

    let client = UsdmOrderClient::new(credentials, UsdmNativeTlsTransport::new())
        .with_base_url(USDM_DEMO_REST_BASE_URL);
    assert_eq!(client.base_url, USDM_DEMO_REST_BASE_URL);

    let mut request =
        UsdmPlaceOrderRequest::market(&symbol, UsdmOrderSide::Buy, demo_usdm_order_qty())
            .with_position_side(demo_usdm_position_side());
    request.new_client_order_id = Some(client_order_id.clone());
    let ack = client
        .place_order(request)
        .await
        .expect("place USDM demo market order");
    assert_eq!(ack.symbol, symbol.to_ascii_uppercase());
    assert_eq!(ack.client_order_id, client_order_id);

    let report = wait_for_usdm_order_reconcile(
        &mut socket,
        &mut hub,
        &mut rx,
        ack.order_id,
        &client_order_id,
    )
    .await;
    let _ = listen_client.close().await;

    assert_eq!(report.symbol, symbol.to_ascii_uppercase());
    assert_eq!(report.client_order_id, client_order_id);
    assert_eq!(report.order_id, ack.order_id);
    assert!(is_terminal_order_status(&report.order_status));
    assert!(
        !hub.writers
            .get(&symbol.to_ascii_uppercase())
            .expect("symbol handler")
            .state()
            .open_orders
            .contains_key(&ack.order_id),
        "terminal USDM order should be cleared by symbol bookkeeping"
    );
}
