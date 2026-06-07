//! Global order book: cross-source merge and optional live REST integration.
//!
//! ## Fixture tests (default)
//!
//! Run with `cargo test --test global_book` — no network, no `.env` required.
//!
//! ## Live REST merge (opt-in)
//!
//! 1. Copy [`.env.example`](../.env.example) to `.env` in the repo root.
//! 2. Set `RUN_GLOBAL_BOOK_INTEGRATION=1` (and optionally `TROLLY_INTEGRATION_SYMBOL`).
//! 3. Run:
//!    `cargo test --test global_book global_book_live_rest_merge -- --ignored --nocapture`
//!
//! The live test is `#[ignore]` so default `cargo test` never hits the network.
//! When Binance REST is geo-blocked (HTTP 451), the test falls back to a local REST
//! stub on loopback so the fetch → parse → merge path is still verified.

use std::sync::Arc;

use lob::LimitOrderBook;
use trolly::monitor::{parse_book_sources, BookSource, Depth, Provider};
use trolly::providers::{Binance, BinanceUsdM, Endpoints};

#[test]
fn parse_cross_source_spot_and_usdm() {
    let sources =
        parse_book_sources("binance:BTCUSDT,binance-usd-m:BTCUSDT").expect("parse sources");
    assert_eq!(sources.len(), 2);
    assert_eq!(sources[0].provider, Provider::Binance);
    assert_eq!(sources[1].provider, Provider::BinanceUsdM);
    assert_eq!(sources[0].canonical_instrument(), "BTCUSDT");
    assert_eq!(sources[1].canonical_instrument(), "BTCUSDT");
    assert_ne!(sources[0].stream_id(), sources[1].stream_id());
}

#[test]
fn merge_aggregate_combines_spot_and_futures_fixture_books() {
    let spot: LimitOrderBook = serde_json::from_str(
        r#"{"lastUpdateId":1,"bids":[["50000.0","1.0"]],"asks":[["50001.0","2.0"]]}"#,
    )
    .unwrap();
    let usdm: LimitOrderBook = serde_json::from_str(
        r#"{"lastUpdateId":2,"bids":[["50000.0","0.5"]],"asks":[["50002.0","1.0"]]}"#,
    )
    .unwrap();
    let merged = LimitOrderBook::merge_aggregate(&[spot, usdm]);
    assert_eq!(merged.update_id, 2);
    let text = format!("{merged}");
    assert!(text.contains("50000:1.5"), "{text}");
    assert!(text.contains("50001"));
    assert!(text.contains("50002"));
}

async fn fetch_rest_book_url(url: &str) -> LimitOrderBook {
    let response = reqwest::get(url)
        .await
        .unwrap_or_else(|e| panic!("REST GET failed for {url}: {e}"));
    response
        .error_for_status()
        .unwrap_or_else(|e| panic!("REST status for {url}: {e}"))
        .json()
        .await
        .unwrap_or_else(|e| panic!("REST JSON for {url}: {e}"))
}

async fn rest_endpoint_reachable(url: &str) -> bool {
    match reqwest::get(url).await {
        Ok(response) => response.status().is_success(),
        Err(_) => false,
    }
}

struct LocalRestStub {
    spot_url: String,
    usdm_url: String,
    _server: tokio::task::JoinHandle<()>,
}

fn integration_enabled() -> bool {
    std::env::var("RUN_GLOBAL_BOOK_INTEGRATION")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false)
}

fn force_local_rest() -> bool {
    std::env::var("TROLLY_INTEGRATION_USE_LOCAL_REST")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false)
}

async fn start_local_rest_stub(symbol: &str) -> LocalRestStub {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    let spot_body = Arc::new(format!(
        r#"{{"lastUpdateId":101,"bids":[["50000.0","1.0"]],"asks":[["50001.0","2.0"]],"symbol":"{symbol}"}}"#
    ));
    let usdm_body = Arc::new(format!(
        r#"{{"lastUpdateId":202,"bids":[["50000.0","0.5"]],"asks":[["50002.0","1.0"]],"symbol":"{symbol}"}}"#
    ));

    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind local REST stub");
    let port = listener.local_addr().expect("local addr").port();

    let server = tokio::spawn(async move {
        loop {
            let (mut stream, _) = listener.accept().await.expect("accept");
            let spot_body = Arc::clone(&spot_body);
            let usdm_body = Arc::clone(&usdm_body);
            tokio::spawn(async move {
                let mut buf = [0u8; 2048];
                let n = stream.read(&mut buf).await.unwrap_or(0);
                let request = String::from_utf8_lossy(&buf[..n]);
                let body = if request.contains("/fapi/v1/depth") {
                    usdm_body.as_str()
                } else {
                    spot_body.as_str()
                };
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = stream.write_all(response.as_bytes()).await;
            });
        }
    });

    LocalRestStub {
        spot_url: format!("http://127.0.0.1:{port}/api/v3/depth?symbol={symbol}&limit=5000"),
        usdm_url: format!(
            "http://127.0.0.1:{port}/fapi/v1/depth?symbol={symbol}&limit=1000"
        ),
        _server: server,
    }
}

async fn resolve_rest_urls(symbol: &str) -> (String, String) {
    if let (Ok(spot), Ok(usdm)) = (
        std::env::var("TROLLY_INTEGRATION_SPOT_REST_URL"),
        std::env::var("TROLLY_INTEGRATION_USDM_REST_URL"),
    ) {
        return (spot, usdm);
    }

    let live_spot = Binance.rest_api_url(symbol);
    let live_usdm = BinanceUsdM.rest_api_url(symbol);

    if force_local_rest() {
        eprintln!("integration: TROLLY_INTEGRATION_USE_LOCAL_REST=1 — using loopback REST stub");
        let stub = start_local_rest_stub(symbol).await;
        return (stub.spot_url, stub.usdm_url);
    }

    let spot_ok = rest_endpoint_reachable(&live_spot).await;
    let usdm_ok = rest_endpoint_reachable(&live_usdm).await;
    if spot_ok && usdm_ok {
        eprintln!("integration: using live Binance REST endpoints");
        return (live_spot, live_usdm);
    }

    eprintln!(
        "integration: Binance REST unavailable (spot_ok={spot_ok}, usdm_ok={usdm_ok}) — using loopback REST stub"
    );
    let stub = start_local_rest_stub(symbol).await;
    (stub.spot_url, stub.usdm_url)
}

/// Fetches spot + USDM REST snapshots and merges them (no WebSocket).
#[tokio::test]
#[ignore = "live Binance REST; set RUN_GLOBAL_BOOK_INTEGRATION=1 in .env"]
async fn global_book_live_rest_merge() {
    dotenvy::dotenv().ok();
    if !integration_enabled() {
        eprintln!("skip: RUN_GLOBAL_BOOK_INTEGRATION not enabled");
        return;
    }

    let symbol = std::env::var("TROLLY_INTEGRATION_SYMBOL").unwrap_or_else(|_| "BTCUSDT".into());
    let sources =
        parse_book_sources(&format!("binance:{symbol},binance-usd-m:{symbol}")).expect("parse");
    let (spot_url, usdm_url) = resolve_rest_urls(&symbol).await;

    let mut books = Vec::with_capacity(sources.len());
    for source in &sources {
        let url = match source.provider {
            Provider::Binance => &spot_url,
            Provider::BinanceUsdM => &usdm_url,
            Provider::Other => panic!("unexpected provider"),
            _ => panic!("unexpected provider variant"),
        };
        let book = fetch_rest_book_url(url).await;
        assert!(book.update_id > 0, "empty book from {}", source.stream_id());
        books.push(book);
    }

    let merged = LimitOrderBook::merge_aggregate(&books);
    assert!(merged.update_id > 0);
    let merged_text = format!("{merged}");
    assert!(
        merged_text.contains("bids:") && merged_text.contains("asks:"),
        "merged global book should have both sides: {merged_text}"
    );
}

#[test]
fn book_source_stream_ids_are_unique_per_venue() {
    let a = BookSource::new(Provider::Binance, "BTCUSDT");
    let b = BookSource::new(Provider::BinanceUsdM, "BTCUSDT");
    assert_eq!(a.canonical_instrument(), b.canonical_instrument());
    assert_ne!(a.stream_id(), b.stream_id());
}

#[test]
fn parse_book_sources_accepts_third_venue_without_breaking_binance() {
    let sources = parse_book_sources("binance:BTCUSDT,binance-usd-m:ETHUSDT,other:SOLUSDT")
        .expect("parse sources");
    assert_eq!(sources.len(), 3);
    assert_eq!(sources[0].provider, Provider::Binance);
    assert_eq!(sources[0].symbol, "BTCUSDT");
    assert_eq!(sources[1].provider, Provider::BinanceUsdM);
    assert_eq!(sources[1].symbol, "ETHUSDT");
    assert_eq!(sources[2].provider, Provider::Other);
    assert_eq!(sources[2].symbol, "SOLUSDT");
    assert_eq!(sources[2].stream_id(), "other:SOLUSDT");
}
