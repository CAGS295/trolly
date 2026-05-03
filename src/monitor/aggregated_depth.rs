//! Multi-symbol depth: one [`crate::monitor::order_book::OrderBook`] per stream id (e.g. `RPI:BTCUSDC` vs `BTCUSDC`)
//! plus a **merged** [`lob::LimitOrderBook`] view refreshed after each delta.
//!
//! Per-symbol books stay authoritative for [`lob::DepthUpdate`] sequencing; the merged book is derived.
//!
//! **Merge semantics (WIP):** [`merge_naive_extend`] concatenates each snapshot’s side vectors via
//! [`lob::LimitOrderBook::extend`]. That does not sum duplicate price levels; it is a placeholder until
//! `lob` exposes level iteration or a purpose-built merge API.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use crate::{providers::Endpoints, EventHandler};

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

struct HubInner {
    factories: HashMap<String, ReadHandleFactory<LimitOrderBook>>,
    merged: WriteHandle<LimitOrderBook, MergedOp>,
    _merged_read: ReadHandle<LimitOrderBook>,
}

/// `Clone` + `Sync` handle for [`MonitorMultiplexor`](crate::connectors::multiplexor::MonitorMultiplexor); all
/// left-right handles live behind [`Mutex`] so this type can cross the multiplexor’s `Context: Sync` bound.
#[derive(Clone)]
pub struct AggregatedDepthHub {
    inner: Arc<Mutex<HubInner>>,
}

impl AggregatedDepthHub {
    pub fn new() -> Self {
        let (mut merged_w, merged_r) = left_right::new::<LimitOrderBook, MergedOp>();
        merged_w.publish();
        Self {
            inner: Arc::new(Mutex::new(HubInner {
                factories: HashMap::new(),
                merged: merged_w,
                _merged_read: merged_r,
            })),
        }
    }

    pub fn merged_factory(&self) -> ReadHandleFactory<LimitOrderBook> {
        self.inner
            .lock()
            .expect("hub lock")
            ._merged_read
            .factory()
    }

    fn register_factory(&self, symbol: String, factory: ReadHandleFactory<LimitOrderBook>) {
        self.inner
            .lock()
            .expect("hub lock")
            .factories
            .insert(symbol, factory);
    }

    fn refresh_merged(&self) {
        let mut inner = self.inner.lock().expect("hub lock");
        let factories: Vec<ReadHandleFactory<LimitOrderBook>> =
            inner.factories.values().cloned().collect();

        let snapshots: Vec<LimitOrderBook> = factories
            .iter()
            .filter_map(|fac| fac.handle().enter().map(|g| (*g).clone()))
            .collect();

        let mut merged = merge_naive_extend(&snapshots);
        merged.update_id = snapshots.iter().map(|b| b.update_id).max().unwrap_or(0);

        inner
            .merged
            .append(MergedOp::Replace(merged))
            .publish();
    }
}

/// One symbol’s live book plus a handle to the shared hub for merged refresh.
pub struct AggregatedDepthShard {
    book: OrderBook,
    hub: AggregatedDepthHub,
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
        self.hub.refresh_merged();
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
        hub.refresh_merged();

        Ok((
            symbol.clone(),
            AggregatedDepthShard {
                book: OrderBook::from(w),
                hub,
            },
        ))
    }
}

/// WebSocket depth for many symbols; each keeps its own book and a merged view is recomputed on every delta.
pub async fn stream_depth_aggregated(provider: super::Provider, symbols: &str) {
    use crate::connectors::multiplexor::MonitorMultiplexor;

    let hub = AggregatedDepthHub::new();
    let symbols = symbols.to_uppercase();
    let syms: Vec<&str> = symbols
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect();

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async move {
            match provider {
                super::Provider::Binance => {
                    MonitorMultiplexor::<AggregatedDepthShard, Depth>::stream::<_, _>(
                        crate::providers::Binance,
                        hub.clone(),
                        &syms,
                    )
                    .await
                }
                super::Provider::BinanceUsdM => {
                    MonitorMultiplexor::<AggregatedDepthShard, Depth>::stream::<_, _>(
                        crate::providers::BinanceUsdM,
                        hub,
                        &syms,
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

#[cfg(test)]
mod tests {
    use super::*;

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
