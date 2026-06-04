//! Prometheus metrics exposed at `GET /metrics` when the `prometheus` feature is enabled.

use prometheus::{CounterVec, Opts, Registry, TextEncoder};
use std::sync::OnceLock;

static REGISTRY: OnceLock<Registry> = OnceLock::new();
static DEPTH_UPDATES: OnceLock<CounterVec> = OnceLock::new();
static MERGE_REFRESHES: OnceLock<CounterVec> = OnceLock::new();

fn registry() -> &'static Registry {
    REGISTRY.get_or_init(|| {
        let registry = Registry::new();

        let depth = CounterVec::new(
            Opts::new("trolly_depth_updates_total", "Depth updates applied to a book shard"),
            &["stream_id"],
        )
        .expect("metric");
        registry
            .register(Box::new(depth.clone()))
            .expect("register depth updates");
        DEPTH_UPDATES.set(depth).expect("set depth updates");

        let merges = CounterVec::new(
            Opts::new(
                "trolly_global_book_merge_refresh_total",
                "Global book merge refreshes per canonical instrument",
            ),
            &["instrument"],
        )
        .expect("metric");
        registry
            .register(Box::new(merges.clone()))
            .expect("register merge refreshes");
        MERGE_REFRESHES.set(merges).expect("set merge refreshes");

        registry
    })
}

pub fn record_depth_update(stream_id: &str) {
    if let Some(counter) = DEPTH_UPDATES.get() {
        counter.with_label_values(&[stream_id]).inc();
    }
}

pub fn record_merge_refresh(instrument: &str) {
    if let Some(counter) = MERGE_REFRESHES.get() {
        counter.with_label_values(&[instrument]).inc();
    }
}

pub fn encode() -> String {
    let encoder = TextEncoder::new();
    let metric_families = registry().gather();
    encoder
        .encode_to_string(&metric_families)
        .unwrap_or_else(|e| format!("# encode error: {e}\n"))
}
