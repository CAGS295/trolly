//! Multi-symbol depth: one [`crate::monitor::order_book::OrderBook`] per stream id (e.g. `RPI:BTCUSDC` vs `BTCUSDC`)
//! plus a **per-instrument merged** [`lob::LimitOrderBook`] refreshed after each delta on any stream in that group.
//!
//! [`canonical_depth_symbol`] strips the `RPI:` prefix so `BTCUSDC` and `RPI:BTCUSDC` share one merge bucket;
//! `ETHBTC` never merges with `BTCUSDC`.
//!
//! **Merge semantics (WIP):** [`merge_naive_extend`] concatenates each snapshot’s side vectors via
//! [`lob::LimitOrderBook::extend`]. That does not sum duplicate price levels; it is a placeholder until
//! `lob` exposes level iteration or a purpose-built merge API.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use crate::{
    providers::{Endpoints, RPI_PREFIX},
    EventHandler,
};

use super::{
    order_book::{Operations, OrderBook},
    Depth,
};
use left_right::{Absorb, ReadHandle, ReadHandleFactory, WriteHandle};
use lob::{DepthUpdate, LimitOrderBook};
use tokio_tungstenite::tungstenite::Message;
use tracing::{instrument, warn};

#[derive(Debug)]
pub(crate) enum MergedOp {
    Replace(LimitOrderBook),
}

impl Absorb<MergedOp> for LimitOrderBook {
    fn absorb_first(&mut self, op: &mut MergedOp, _other: &Self) {
        match op {
            MergedOp::Replace(next) => {
                *self = std::mem::replace(next, LimitOrderBook::default());
            }
        }
    }

    fn sync_with(&mut self, first: &Self) {
        *self = first.clone();
    }
}

/// Concatenate books with [`LimitOrderBook::extend`] (ordering / duplicate prices not reconciled).
pub fn merge_naive_extend(books: &[LimitOrderBook]) -> LimitOrderBook {
    let mut out = LimitOrderBook::new();
    for b in books {
        out.extend(b);
    }
    out
}

/// Instrument key shared by `PAIR` and `RPI:PAIR` depth streams (uppercase, no `RPI:` prefix).
pub fn canonical_depth_symbol(symbol: &str) -> String {
    symbol
        .trim()
        .strip_prefix(RPI_PREFIX)
        .unwrap_or(symbol.trim())
        .to_uppercase()
}

struct MergedLane {
    merged: WriteHandle<LimitOrderBook, MergedOp>,
    _merged_read: ReadHandle<LimitOrderBook>,
}

struct HubInner {
    factories: HashMap<String, ReadHandleFactory<LimitOrderBook>>,
    merged_by_canonical: HashMap<String, MergedLane>,
}

/// `Clone` + `Sync` handle for [`MonitorMultiplexor`](crate::connectors::multiplexor::MonitorMultiplexor); all
/// left-right handles live behind [`Mutex`] so this type can cross the multiplexor’s `Context: Sync` bound.
#[derive(Clone)]
pub struct AggregatedDepthHub {
    inner: Arc<Mutex<HubInner>>,
}

impl AggregatedDepthHub {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(HubInner {
                factories: HashMap::new(),
                merged_by_canonical: HashMap::new(),
            })),
        }
    }

    /// Merged book for one instrument (`BTCUSDC` aggregates `BTCUSDC` + `RPI:BTCUSDC` factories only).
    pub fn merged_factory_for(&self, canonical: &str) -> Option<ReadHandleFactory<LimitOrderBook>> {
        let inner = self.inner.lock().expect("hub lock");
        let c = canonical.to_uppercase();
        inner
            .merged_by_canonical
            .get(&c)
            .map(|lane| lane._merged_read.factory())
    }

    /// Snapshot handles for each live per-symbol book (for inspection or UIs).
    pub fn per_symbol_factories(&self) -> Vec<(String, ReadHandleFactory<LimitOrderBook>)> {
        let inner = self.inner.lock().expect("hub lock");
        inner
            .factories
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect()
    }

    fn register_factory(&self, symbol: String, factory: ReadHandleFactory<LimitOrderBook>) {
        let mut inner = self.inner.lock().expect("hub lock");
        let canon = canonical_depth_symbol(&symbol);
        inner.factories.insert(symbol, factory);
        use std::collections::hash_map::Entry;
        if let Entry::Vacant(e) = inner.merged_by_canonical.entry(canon.clone()) {
            let (mut w, r) = left_right::new::<LimitOrderBook, MergedOp>();
            w.publish();
            e.insert(MergedLane {
                merged: w,
                _merged_read: r,
            });
        }
    }

    fn refresh_merged_for(&self, canonical: &str) {
        let c = canonical.to_uppercase();
        let group: Vec<ReadHandleFactory<LimitOrderBook>> = {
            let inner = self.inner.lock().expect("hub lock");
            inner
                .factories
                .iter()
                .filter(|(sym, _)| canonical_depth_symbol(sym) == c)
                .map(|(_, fac)| fac.clone())
                .collect()
        };
        let snapshots: Vec<LimitOrderBook> = group
            .iter()
            .filter_map(|fac| fac.handle().enter().map(|g| (*g).clone()))
            .collect();
        let mut merged = merge_naive_extend(&snapshots);
        merged.update_id = snapshots.iter().map(|b| b.update_id).max().unwrap_or(0);
        let mut inner = self.inner.lock().expect("hub lock");
        let Some(lane) = inner.merged_by_canonical.get_mut(&c) else {
            return;
        };
        lane.merged
            .append(MergedOp::Replace(merged))
            .publish();
    }
}

/// One symbol’s live book plus a handle to the shared hub for merged refresh.
pub struct AggregatedDepthShard {
    book: OrderBook,
    hub: AggregatedDepthHub,
    canonical: String,
}

impl EventHandler<Depth> for AggregatedDepthShard {
    type Error = color_eyre::eyre::Error;
    type Context = AggregatedDepthHub;
    type Update = DepthUpdate;

    fn parse_update(value: Message) -> Result<Option<Self::Update>, Self::Error> {
        OrderBook::parse_update(value)
    }

    fn to_id(update: &DepthUpdate) -> &str {
        OrderBook::to_id(update)
    }

    #[instrument(skip_all, fields(pair))]
    fn handle_update(&mut self, update: DepthUpdate) -> Result<(), Self::Error> {
        EventHandler::handle_update(&mut self.book, update)?;
        self.hub.refresh_merged_for(&self.canonical);
        Ok(())
    }

    async fn build<En>(
        provider: En,
        symbols: &[impl AsRef<str>],
        hub: Self::Context,
    ) -> Result<(String, Self), Self::Error>
    where
        En: Endpoints<Depth>,
    {
        assert_eq!(symbols.len(), 1);
        let symbol = symbols.first().expect("one symbol").as_ref().to_string();
        let canonical = canonical_depth_symbol(&symbol);

        let response: reqwest::Response = {
            let url = provider.rest_api_url(&symbol);
            reqwest::get(url).await
        }?;
        if let Err(e) = response.error_for_status_ref() {
            return Err(color_eyre::eyre::eyre!(
                "REST snapshot failed: {e} {}",
                response.text().await?
            ));
        }
        let lob: LimitOrderBook = response.json().await?;

        let (mut w, r) = left_right::new::<LimitOrderBook, Operations>();
        let factory = r.factory();
        hub.register_factory(symbol.clone(), factory);

        w.append(Operations::Initialize(lob));
        hub.refresh_merged_for(&canonical);

        Ok((
            symbol.clone(),
            AggregatedDepthShard {
                book: OrderBook::from(w),
                hub,
                canonical,
            },
        ))
    }
}

/// Run the multiplexed WebSocket stream with an existing hub (e.g. shared with a TUI).
pub async fn run_aggregated_depth_stream(
    hub: AggregatedDepthHub,
    provider: super::Provider,
    syms: &[&str],
) {
    use crate::connectors::multiplexor::MonitorMultiplexor;

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async move {
            match provider {
                super::Provider::Binance => {
                    MonitorMultiplexor::<AggregatedDepthShard, Depth>::stream::<_, _>(
                        crate::providers::Binance,
                        hub.clone(),
                        syms,
                    )
                    .await
                }
                super::Provider::BinanceUsdM => {
                    MonitorMultiplexor::<AggregatedDepthShard, Depth>::stream::<_, _>(
                        crate::providers::BinanceUsdM,
                        hub,
                        syms,
                    )
                    .await
                }
                super::Provider::Other => {
                    warn!("aggregated depth: unknown provider");
                }
            }
        })
        .await;
}

/// WebSocket depth for many symbols; each keeps its own book and a merged view is recomputed on every delta.
pub async fn stream_depth_aggregated(provider: super::Provider, symbols: &str) {
    let hub = AggregatedDepthHub::new();
    let symbols = symbols.to_uppercase();
    let syms: Vec<&str> = symbols
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect();
    run_aggregated_depth_stream(hub, provider, &syms).await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_depth_symbol_groups_rpi() {
        assert_eq!(canonical_depth_symbol("RPI:BTCUSDC"), "BTCUSDC");
        assert_eq!(canonical_depth_symbol("btcusdc"), "BTCUSDC");
        assert_eq!(canonical_depth_symbol("ETHBTC"), "ETHBTC");
    }

    #[test]
    fn merge_naive_extend_empty() {
        let m = merge_naive_extend(&[]);
        assert_eq!(m.update_id, 0);
    }

    #[test]
    fn merge_naive_extend_preserves_levels_via_display() {
        let b1: LimitOrderBook = serde_json::from_str(
            r#"{"lastUpdateId":1,"bids":[["100.0","1.0"]],"asks":[["101.0","2.0"]]}"#,
        )
        .unwrap();
        let b2: LimitOrderBook = serde_json::from_str(
            r#"{"lastUpdateId":2,"bids":[["99.5","3.0"]],"asks":[["102.0","1.0"]]}"#,
        )
        .unwrap();
        let m = merge_naive_extend(&[b1, b2]);
        let s = format!("{m}");
        assert!(s.contains("100"));
        assert!(s.contains("99.5"));
    }
}
