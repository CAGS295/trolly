//! Shared tick schema for ClickHouse ingest and training-tape construction.
//!
//! Advertise and fulfill use the same [`TickRow`]: insert, query, and
//! [`TickRow::to_stream_event`] / [`features_from_event`] must stay aligned
//! with [`crate::observation::depth_features`].

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use trolly_strategy::{DepthUpdate, PriceLevel, StreamEvent};

use crate::observation::{features_from_event, FeatureVector};

/// Default local HTTP endpoint (docker-compose in `clickhouse/`).
pub const DEFAULT_CLICKHOUSE_URL: &str = "http://127.0.0.1:8123";
pub const DEFAULT_DATABASE: &str = "trolly";
pub const DEFAULT_TABLE: &str = "ticks";

/// One book/trade tick. Columns match `clickhouse/init.sql`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TickRow {
    #[serde(deserialize_with = "de_i64_flex")]
    pub ts_ms: i64,
    pub venue: String,
    pub symbol: String,
    pub event_kind: String,
    pub side: String,
    pub price: f64,
    pub size: f64,
    pub bid: f64,
    pub ask: f64,
    pub bid_qty: f64,
    pub ask_qty: f64,
    pub mid: f64,
    pub spread: f64,
    #[serde(deserialize_with = "de_u64_flex")]
    pub update_id: u64,
    pub source: String,
    pub session_id: String,
}

fn de_i64_flex<'de, D: serde::Deserializer<'de>>(d: D) -> Result<i64, D::Error> {
    use serde::de::{self, Visitor};
    struct V;
    impl<'de> Visitor<'de> for V {
        type Value = i64;
        fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
            f.write_str("i64 or numeric string")
        }
        fn visit_i64<E: de::Error>(self, v: i64) -> Result<i64, E> {
            Ok(v)
        }
        fn visit_u64<E: de::Error>(self, v: u64) -> Result<i64, E> {
            Ok(v as i64)
        }
        fn visit_str<E: de::Error>(self, v: &str) -> Result<i64, E> {
            v.parse().map_err(E::custom)
        }
    }
    d.deserialize_any(V)
}

fn de_u64_flex<'de, D: serde::Deserializer<'de>>(d: D) -> Result<u64, D::Error> {
    use serde::de::{self, Visitor};
    struct V;
    impl<'de> Visitor<'de> for V {
        type Value = u64;
        fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
            f.write_str("u64 or numeric string")
        }
        fn visit_u64<E: de::Error>(self, v: u64) -> Result<u64, E> {
            Ok(v)
        }
        fn visit_i64<E: de::Error>(self, v: i64) -> Result<u64, E> {
            Ok(v as u64)
        }
        fn visit_str<E: de::Error>(self, v: &str) -> Result<u64, E> {
            v.parse().map_err(E::custom)
        }
    }
    d.deserialize_any(V)
}

impl TickRow {
    /// Build from a depth event using the same top-of-book parse as gym features.
    pub fn from_depth_event(
        event: &StreamEvent,
        venue: &str,
        source: &str,
        session_id: &str,
        ts_ms: i64,
    ) -> Option<Self> {
        let StreamEvent::Depth(depth) = event else {
            return None;
        };
        let features = features_from_event(event)?;
        let f = features.as_slice();
        let bid = *f.first()? as f64;
        let bid_qty = *f.get(1)? as f64;
        let ask = *f.get(2)? as f64;
        let ask_qty = *f.get(3)? as f64;
        let spread = *f.get(4)? as f64;
        let mid = *f.get(5)? as f64;
        let update_id = depth.update_id.unwrap_or(0);
        Some(Self {
            ts_ms,
            venue: venue.into(),
            symbol: depth.symbol.clone(),
            event_kind: "depth".into(),
            side: "book".into(),
            price: mid,
            size: bid_qty + ask_qty,
            bid,
            ask,
            bid_qty,
            ask_qty,
            mid,
            spread,
            update_id,
            source: source.into(),
            session_id: session_id.into(),
        })
    }

    /// Rebuild the stream event the gym feature extractor already understands.
    pub fn to_stream_event(&self) -> StreamEvent {
        StreamEvent::Depth(DepthUpdate {
            symbol: self.symbol.clone(),
            bids: vec![PriceLevel {
                price: format!("{:.4}", self.bid),
                qty: format!("{:.4}", self.bid_qty),
            }],
            asks: vec![PriceLevel {
                price: format!("{:.4}", self.ask),
                qty: format!("{:.4}", self.ask_qty),
            }],
            update_id: Some(self.update_id),
        })
    }

    /// Features via [`features_from_event`] — same path as live `Env` ingest.
    pub fn to_features(&self) -> Option<FeatureVector> {
        features_from_event(&self.to_stream_event())
    }
}

/// Ordered mid/book path reconstructed from stored ticks (training tape).
#[derive(Debug, Clone, Default)]
pub struct TickTape {
    pub rows: Vec<TickRow>,
    idx: usize,
}

impl TickTape {
    pub fn new(rows: Vec<TickRow>) -> Self {
        Self { rows, idx: 0 }
    }

    pub fn len(&self) -> usize {
        self.rows.len()
    }

    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    pub fn reset(&mut self) {
        self.idx = 0;
    }

    pub fn next(&mut self) -> Option<TickRow> {
        let row = self.rows.get(self.idx).cloned()?;
        self.idx += 1;
        Some(row)
    }

    pub fn remaining(&self) -> usize {
        self.rows.len().saturating_sub(self.idx)
    }

    /// SHA-256 of the session window (fingerprint data-window join).
    pub fn window_hash(&self) -> String {
        crate::fingerprint::hash_bytes(&tick_window_bytes(&self.rows))
    }
}

fn tick_window_bytes(rows: &[TickRow]) -> Vec<u8> {
    let mut out = Vec::new();
    for row in rows {
        out.extend_from_slice(row.session_id.as_bytes());
        out.extend_from_slice(&row.ts_ms.to_le_bytes());
        out.extend_from_slice(row.symbol.as_bytes());
        out.extend_from_slice(&row.mid.to_le_bytes());
        out.extend_from_slice(&row.bid.to_le_bytes());
        out.extend_from_slice(&row.ask.to_le_bytes());
        out.extend_from_slice(&row.update_id.to_le_bytes());
    }
    out
}

pub fn now_ts_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// HTTP ClickHouse client (JSONEachRow). Same schema for insert and select.
#[derive(Debug, Clone)]
pub struct ClickHouseTicks {
    pub url: String,
    pub database: String,
    pub table: String,
}

impl Default for ClickHouseTicks {
    fn default() -> Self {
        Self {
            url: std::env::var("TROLLY_CLICKHOUSE_URL")
                .unwrap_or_else(|_| DEFAULT_CLICKHOUSE_URL.into()),
            database: DEFAULT_DATABASE.into(),
            table: DEFAULT_TABLE.into(),
        }
    }
}

impl ClickHouseTicks {
    pub fn ping(&self) -> Result<(), String> {
        let body = http_get(&format!("{}/ping", self.url.trim_end_matches('/')))?;
        if body.trim() == "Ok." || body.contains("Ok") {
            Ok(())
        } else {
            Err(format!("unexpected ping body: {body}"))
        }
    }

    pub fn ensure_schema(&self) -> Result<(), String> {
        self.query(&format!("CREATE DATABASE IF NOT EXISTS {}", self.database))?;
        self.query(&format!(
            "CREATE TABLE IF NOT EXISTS {}.{} (
                ts_ms Int64,
                venue LowCardinality(String),
                symbol LowCardinality(String),
                event_kind LowCardinality(String),
                side LowCardinality(String),
                price Float64,
                size Float64,
                bid Float64,
                ask Float64,
                bid_qty Float64,
                ask_qty Float64,
                mid Float64,
                spread Float64,
                update_id UInt64,
                source LowCardinality(String),
                session_id String
            ) ENGINE = MergeTree
            ORDER BY (symbol, ts_ms, session_id)",
            self.database, self.table
        ))?;
        Ok(())
    }

    pub fn insert(&self, rows: &[TickRow]) -> Result<usize, String> {
        if rows.is_empty() {
            return Ok(0);
        }
        let mut body = String::new();
        for row in rows {
            body.push_str(&serde_json::to_string(row).map_err(|e| e.to_string())?);
            body.push('\n');
        }
        let q = format!(
            "INSERT INTO {}.{} FORMAT JSONEachRow",
            self.database, self.table
        );
        http_post_query(&self.url, &q, &body)?;
        Ok(rows.len())
    }

    pub fn query_session(&self, session_id: &str) -> Result<Vec<TickRow>, String> {
        let q = format!(
            "SELECT ts_ms, venue, symbol, event_kind, side, price, size, bid, ask, \
             bid_qty, ask_qty, mid, spread, update_id, source, session_id \
             FROM {}.{} WHERE session_id = '{}' ORDER BY ts_ms ASC FORMAT JSONEachRow",
            self.database,
            self.table,
            session_id.replace('\'', "")
        );
        parse_json_each_row(&self.query(&q)?)
    }

    pub fn query_recent(&self, symbol: &str, limit: usize) -> Result<Vec<TickRow>, String> {
        let q = format!(
            "SELECT ts_ms, venue, symbol, event_kind, side, price, size, bid, ask, \
             bid_qty, ask_qty, mid, spread, update_id, source, session_id \
             FROM {}.{} WHERE symbol = '{}' ORDER BY ts_ms DESC LIMIT {} FORMAT JSONEachRow",
            self.database,
            self.table,
            symbol.replace('\'', ""),
            limit
        );
        let mut rows = parse_json_each_row(&self.query(&q)?)?;
        rows.reverse();
        Ok(rows)
    }

    pub fn query(&self, sql: &str) -> Result<String, String> {
        http_post_query(&self.url, sql, "")
    }
}

fn parse_json_each_row(body: &str) -> Result<Vec<TickRow>, String> {
    let mut rows = Vec::new();
    for line in body.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        rows.push(serde_json::from_str(line).map_err(|e| format!("tick json: {e}: {line}"))?);
    }
    Ok(rows)
}

fn http_get(url: &str) -> Result<String, String> {
    let resp = ureq::get(url)
        .timeout(Duration::from_secs(5))
        .call()
        .map_err(|e| e.to_string())?;
    read_body(resp)
}

fn http_post_query(base: &str, sql: &str, body: &str) -> Result<String, String> {
    let url = format!(
        "{}/?query={}",
        base.trim_end_matches('/'),
        urlencoding_lite(sql)
    );
    let resp = ureq::post(&url)
        .timeout(Duration::from_secs(15))
        .set("Content-Type", "application/json")
        .send_string(body)
        .map_err(|e| e.to_string())?;
    read_body(resp)
}

fn read_body(resp: ureq::Response) -> Result<String, String> {
    let mut buf = String::new();
    resp.into_reader()
        .read_to_string(&mut buf)
        .map_err(|e| e.to_string())?;
    Ok(buf)
}

fn urlencoding_lite(raw: &str) -> String {
    let mut out = String::new();
    for b in raw.as_bytes() {
        match *b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(*b as char);
            }
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

/// Generate sim ticks, insert, then reload the same session (ingest ↔ train join).
pub fn ingest_and_reload_sim(
    client: &ClickHouseTicks,
    config: &crate::sim::MicrostructureConfig,
    steps: usize,
) -> Result<TickTape, String> {
    client.ensure_schema()?;
    let session_id = format!("sim-{}", now_ts_ms());
    let rows = crate::sim::ingest_sim_ticks(config, &session_id, steps);
    client.insert(&rows)?;
    let loaded = client.query_session(&session_id)?;
    if loaded.is_empty() {
        return Err("ClickHouse query returned no ticks after insert".into());
    }
    Ok(TickTape::new(loaded))
}

/// Best-effort stream ingest. Enabled when `TROLLY_INGEST_TICKS=1` or
/// `TROLLY_CLICKHOUSE_URL` is set. Never blocks the gym if ClickHouse is down.
pub fn try_ingest_event(event: &StreamEvent, venue: &str, source: &str) {
    if !ingest_enabled() {
        return;
    }
    let client = ClickHouseTicks::default();
    if client.ping().is_err() {
        return;
    }
    let _ = client.ensure_schema();
    if let Some(row) = TickRow::from_depth_event(event, venue, source, "stream", now_ts_ms()) {
        let _ = client.insert(&[row]);
    }
}

fn ingest_enabled() -> bool {
    std::env::var("TROLLY_INGEST_TICKS")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false)
        || std::env::var("TROLLY_CLICKHOUSE_URL").is_ok()
}

/// Error when the weekday trainer cannot talk to ClickHouse.
pub fn clickhouse_unreachable_error(url: &str, err: impl std::fmt::Display) -> String {
    format!("training bails: ClickHouse is not reachable at {url} ({err})")
}

/// Ping the configured ClickHouse URL. Does not start Docker.
///
/// The weekday trainer must call this (or [`ensure_local_clickhouse`]) and
/// refuse to train on an in-memory tape when the database is down.
pub fn require_clickhouse_reachable() -> Result<ClickHouseTicks, String> {
    let client = ClickHouseTicks::default();
    client
        .ping()
        .map_err(|err| clickhouse_unreachable_error(&client.url, err))?;
    Ok(client)
}

/// Ping, or start `clickhouse/docker-compose.yml` and wait.
pub fn ensure_local_clickhouse() -> Result<ClickHouseTicks, String> {
    let client = ClickHouseTicks::default();
    if client.ping().is_ok() {
        client.ensure_schema()?;
        return Ok(client);
    }
    let compose = locate_compose_file().ok_or_else(|| {
        "ClickHouse is down and docker-compose.yml was not found \
         (expected crates/trolly-gym/clickhouse/docker-compose.yml)"
            .to_string()
    })?;
    let status = Command::new("docker")
        .args([
            "compose",
            "-f",
            &compose.display().to_string(),
            "up",
            "-d",
        ])
        .status()
        .map_err(|e| format!("docker compose: {e}"))?;
    if !status.success() {
        return Err("docker compose up failed (is Docker running?)".into());
    }
    for _ in 0..40 {
        thread::sleep(Duration::from_millis(500));
        if client.ping().is_ok() {
            client.ensure_schema()?;
            return Ok(client);
        }
    }
    Err("ClickHouse did not become ready on http://127.0.0.1:8123".into())
}

fn locate_compose_file() -> Option<PathBuf> {
    let candidates = [
        PathBuf::from("crates/trolly-gym/clickhouse/docker-compose.yml"),
        PathBuf::from("clickhouse/docker-compose.yml"),
        Path::new(env!("CARGO_MANIFEST_DIR")).join("clickhouse/docker-compose.yml"),
    ];
    candidates.into_iter().find(|p| p.exists())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_event() -> StreamEvent {
        StreamEvent::Depth(DepthUpdate {
            symbol: "SYNTHUSDT".into(),
            bids: vec![PriceLevel {
                price: "99.5000".into(),
                qty: "1.0000".into(),
            }],
            asks: vec![PriceLevel {
                price: "100.5000".into(),
                qty: "1.0000".into(),
            }],
            update_id: Some(7),
        })
    }

    #[test]
    fn tick_row_roundtrips_through_feature_extractor() {
        let event = sample_event();
        let row = TickRow::from_depth_event(&event, "sim", "sim", "sess-1", 1).unwrap();
        let again = row.to_features().unwrap();
        let original = features_from_event(&event).unwrap();
        assert_eq!(again.as_slice()[5], original.as_slice()[5]); // mid
        assert_eq!(again.as_slice()[4], original.as_slice()[4]); // spread
        assert_eq!(row.symbol, "SYNTHUSDT");
        assert_eq!(row.update_id, 7);
    }

    #[test]
    fn clickhouse_insert_and_query_when_running() {
        let ch = ClickHouseTicks::default();
        if ch.ping().is_err() {
            return;
        }
        ch.ensure_schema().expect("schema");
        let event = sample_event();
        let mut row = TickRow::from_depth_event(&event, "sim", "sim", "test-roundtrip", 1).unwrap();
        row.session_id = format!("test-roundtrip-{}", now_ts_ms());
        let n = ch.insert(&[row.clone()]).expect("insert");
        assert_eq!(n, 1);
        let got = ch.query_session(&row.session_id).expect("query");
        assert_eq!(got.len(), 1);
        assert!((got[0].mid - row.mid).abs() < 1e-6);
    }

    #[test]
    fn require_clickhouse_bails_on_dead_url() {
        let client = ClickHouseTicks {
            url: "http://127.0.0.1:1".into(),
            database: DEFAULT_DATABASE.into(),
            table: DEFAULT_TABLE.into(),
        };
        let err = clickhouse_unreachable_error(
            &client.url,
            client.ping().expect_err("port 1 must refuse"),
        );
        assert!(err.contains("training bails: ClickHouse is not reachable"));
        assert!(err.contains("http://127.0.0.1:1"));
    }

    #[test]
    fn tape_window_hash_is_stable() {
        let event = sample_event();
        let row = TickRow::from_depth_event(&event, "sim", "sim", "sess-1", 1).unwrap();
        let a = TickTape::new(vec![row.clone()]).window_hash();
        let b = TickTape::new(vec![row]).window_hash();
        assert_eq!(a, b);
        assert_eq!(a.len(), 64);
    }
}
