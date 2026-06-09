//! Global order book: cross-source merge and optional live REST integration.
//!
//! ## Default test run (no network)
//!
//! `cargo test` always runs the fixture tests below and skips the live REST test because it is
//! marked `#[ignore]`.
//!
//! ## Live REST merge test (opt-in)
//!
//! 1. Copy [`.env.example`](../.env.example) to `.env` in the repo root.
//! 2. Set `RUN_GLOBAL_BOOK_INTEGRATION=1` in `.env`.
//! 3. Run:
//!    `cargo test --test global_book global_book_live_rest_merge -- --ignored --nocapture`
//!
//! Without step 2, the ignored test exits early (no network calls). See README.md for details.

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

fn global_book_live_integration_enabled() -> bool {
    std::env::var("RUN_GLOBAL_BOOK_INTEGRATION")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false)
}

async fn fetch_rest_book<E: Endpoints<Depth>>(endpoint: E, symbol: &str) -> LimitOrderBook {
    let url = endpoint.rest_api_url(symbol);
    let response = reqwest::get(url)
        .await
        .unwrap_or_else(|e| panic!("REST GET failed: {e}"));
    response
        .error_for_status()
        .expect("REST status")
        .json()
        .await
        .expect("REST JSON")
}

/// Fetches Binance spot + USDM REST snapshots and merges them (no WebSocket).
///
/// Ignored by default; enable via `.env` (see module docs and `.env.example`).
#[tokio::test]
#[ignore = "live Binance REST; copy .env.example → .env and set RUN_GLOBAL_BOOK_INTEGRATION=1"]
async fn global_book_live_rest_merge() {
    dotenvy::dotenv().ok();
    if !global_book_live_integration_enabled() {
        eprintln!(
            "skip: RUN_GLOBAL_BOOK_INTEGRATION not enabled (copy .env.example → .env and set to 1)"
        );
        return;
    }

    let symbol = std::env::var("TROLLY_INTEGRATION_SYMBOL").unwrap_or_else(|_| "BTCUSDT".into());
    let sources =
        parse_book_sources(&format!("binance:{symbol},binance-usd-m:{symbol}")).expect("parse");

    let mut books = Vec::with_capacity(sources.len());
    for source in &sources {
        let book = match &source.provider {
            Provider::Binance => fetch_rest_book(Binance, &source.symbol).await,
            Provider::BinanceUsdM => fetch_rest_book(BinanceUsdM, &source.symbol).await,
            Provider::Custom(label) => {
                panic!("unexpected custom provider {label:?} in integration test")
            }
            _ => panic!("unexpected provider variant"),
        };
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
