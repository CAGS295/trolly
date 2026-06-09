//! Global order book: cross-source merge and optional live REST integration.
//!
//! # Running tests
//!
//! **Default (no network):** `cargo test --test global_book` runs fixture-only tests.
//!
//! **Live REST merge:** copy [`.env.example`](../.env.example) to `.env`, set
//! `RUN_GLOBAL_BOOK_INTEGRATION=1`, then:
//!
//! ```bash
//! cargo test --test global_book global_book_live_rest_merge -- --ignored --nocapture
//! ```

use lob::LimitOrderBook;
use std::sync::Once;
use trolly::monitor::{parse_book_sources, BookSource, Depth, Provider};
use trolly::providers::{Binance, BinanceUsdM, Endpoints};

static LOAD_DOTENV: Once = Once::new();

fn load_integration_env() {
    LOAD_DOTENV.call_once(|| {
        dotenvy::dotenv().ok();
    });
}

fn global_book_integration_enabled() -> bool {
    load_integration_env();
    std::env::var("RUN_GLOBAL_BOOK_INTEGRATION")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false)
}

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
#[tokio::test]
#[ignore = "live Binance REST; set RUN_GLOBAL_BOOK_INTEGRATION=1 in .env (see .env.example)"]
async fn global_book_live_rest_merge() {
    if !global_book_integration_enabled() {
        eprintln!("skip: RUN_GLOBAL_BOOK_INTEGRATION not enabled (see .env.example)");
        return;
    }

    let symbol = std::env::var("TROLLY_INTEGRATION_SYMBOL").unwrap_or_else(|_| "BTCUSDT".into());
    let sources =
        parse_book_sources(&format!("binance:{symbol},binance-usd-m:{symbol}")).expect("parse");

    let mut books = Vec::with_capacity(sources.len());
    for source in &sources {
        let book = match source.provider {
            Provider::Binance => fetch_rest_book(Binance, &source.symbol).await,
            Provider::BinanceUsdM => fetch_rest_book(BinanceUsdM, &source.symbol).await,
            Provider::Registered(_) => panic!("unexpected provider"),
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
