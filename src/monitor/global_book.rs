//! Cross-source **global order book**: one live book per [`BookSource`], merged by
//! [`BookSource::canonical_instrument`] (e.g. `binance:BTCUSDT` + `binance-usd-m:BTCUSDT` → `BTCUSDT`).
//!
//! Intra-provider overlays (Binance USDM `@depth` vs `@rpiDepth`) are **not** folded into the
//! canonical instrument unless you subscribe both under the same symbol name; RPI remains a
//! venue-specific stream id (`binance-usd-m:RPI:BTCUSDT`).

use std::collections::HashMap;
use std::ops::Deref;
use std::sync::{Arc, Mutex};

use crate::EventHandler;
use trolly_stream::VenueEndpoints;

use super::{
    order_book::{Operations, OrderBook},
    parse_book_sources, BookSource, Depth, Provider,
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
        self.replace_from(first);
    }
}

struct MergedLane {
    merged: WriteHandle<LimitOrderBook, MergedOp>,
    _merged_read: ReadHandle<LimitOrderBook>,
}

struct HubInner {
    factories: HashMap<String, ReadHandleFactory<LimitOrderBook>>,
    stream_instruments: HashMap<String, String>,
    sources_by_instrument: HashMap<String, Vec<String>>,
    merged_by_instrument: HashMap<String, MergedLane>,
}

/// Shared hub for all cross-source depth streams.
#[derive(Clone)]
pub struct GlobalBookHub {
    inner: Arc<Mutex<HubInner>>,
    #[cfg(any(feature = "codec", feature = "grpc"))]
    serve: Option<crate::servers::BookRegistry>,
}

impl GlobalBookHub {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(HubInner {
                factories: HashMap::new(),
                stream_instruments: HashMap::new(),
                sources_by_instrument: HashMap::new(),
                merged_by_instrument: HashMap::new(),
            })),
            #[cfg(any(feature = "codec", feature = "grpc"))]
            serve: None,
        }
    }

    /// Hub that registers merged canonical instruments on `registry` for gRPC / SCALE lookup.
    #[cfg(any(feature = "codec", feature = "grpc"))]
    pub fn with_serve_registry(registry: crate::servers::BookRegistry) -> Self {
        Self {
            inner: Arc::new(Mutex::new(HubInner {
                factories: HashMap::new(),
                stream_instruments: HashMap::new(),
                sources_by_instrument: HashMap::new(),
                merged_by_instrument: HashMap::new(),
            })),
            serve: Some(registry),
        }
    }

    #[cfg(any(feature = "codec", feature = "grpc"))]
    fn publish_serve_instrument(&self, instrument: &str) {
        let Some(registry) = &self.serve else {
            return;
        };
        let Some(factory) = self.merged_factory_for(instrument) else {
            return;
        };
        registry
            .lock()
            .expect("book registry lock")
            .insert(instrument.to_uppercase(), factory);
    }

    /// Merged book for one instrument (all [`BookSource`]s sharing [`BookSource::canonical_instrument`]).
    pub fn merged_factory_for(&self, instrument: &str) -> Option<ReadHandleFactory<LimitOrderBook>> {
        let inner = self.inner.lock().expect("hub lock");
        inner
            .merged_by_instrument
            .get(&instrument.to_uppercase())
            .map(|lane| lane._merged_read.factory())
    }

    /// Snapshot handles keyed by [`BookSource::stream_id`].
    pub fn per_source_factories(&self) -> Vec<(String, ReadHandleFactory<LimitOrderBook>)> {
        let inner = self.inner.lock().expect("hub lock");
        inner
            .factories
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect()
    }

    fn register_factory(
        &self,
        stream_id: String,
        factory: ReadHandleFactory<LimitOrderBook>,
        instrument: &str,
    ) {
        let mut inner = self.inner.lock().expect("hub lock");
        let key = instrument.to_uppercase();
        inner
            .stream_instruments
            .insert(stream_id.clone(), key.clone());
        inner
            .sources_by_instrument
            .entry(key.clone())
            .or_default()
            .push(stream_id.clone());
        inner.factories.insert(stream_id, factory);
        use std::collections::hash_map::Entry;
        if let Entry::Vacant(e) = inner.merged_by_instrument.entry(key.clone()) {
            let (mut w, r) = left_right::new::<LimitOrderBook, MergedOp>();
            w.publish();
            e.insert(MergedLane {
                merged: w,
                _merged_read: r,
            });
        }
        drop(inner);
        #[cfg(any(feature = "codec", feature = "grpc"))]
        self.publish_serve_instrument(instrument);
    }

    fn refresh_merged_for(&self, instrument: &str) {
        let key = instrument.to_uppercase();
        let group: Vec<ReadHandleFactory<LimitOrderBook>> = {
            let inner = self.inner.lock().expect("hub lock");
            inner
                .sources_by_instrument
                .get(&key)
                .into_iter()
                .flatten()
                .filter_map(|stream_id| inner.factories.get(stream_id).cloned())
                .collect()
        };
        if group.is_empty() {
            return;
        }

        let merged = if group.len() == 1 {
            group[0]
                .handle()
                .enter()
                .map(|guard| guard.clone())
                .unwrap_or_default()
        } else {
            let mut merged = LimitOrderBook::new();
            for factory in &group {
                if let Some(book) = factory.handle().enter() {
                    merged.merge_aggregate_absorb(&book);
                }
            }
            merged
        };

        let mut merged = merged;
        merged.update_id = group
            .iter()
            .filter_map(|fac| fac.handle().enter().map(|g| g.update_id))
            .max()
            .unwrap_or(0);

        let mut inner = self.inner.lock().expect("hub lock");
        let Some(lane) = inner.merged_by_instrument.get_mut(&key) else {
            return;
        };
        lane.merged
            .append(MergedOp::Replace(merged))
            .publish();

        #[cfg(feature = "prometheus")]
        crate::servers::metrics::record_merge_refresh(&key);
    }
}

/// One subscribed symbol on one provider, wired into a shared [`GlobalBookHub`].
pub struct GlobalBookShard {
    book: OrderBook,
    hub: GlobalBookHub,
    instrument: String,
    stream_id: String,
}

impl EventHandler<Depth> for GlobalBookShard {
    type Error = color_eyre::eyre::Error;
    type Context = (GlobalBookHub, Provider);
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
        self.hub.refresh_merged_for(&self.instrument);
        #[cfg(feature = "prometheus")]
        crate::servers::metrics::record_depth_update(&self.stream_id);
        Ok(())
    }

    async fn build<En>(
        provider: En,
        symbols: &[impl AsRef<str>],
        (hub, venue): Self::Context,
    ) -> Result<(String, Self), Self::Error>
    where
        En: VenueEndpoints,
    {
        assert_eq!(symbols.len(), 1);
        let symbol = symbols.first().expect("one symbol").as_ref().to_string();
        let source = BookSource::new(venue, &symbol);
        let stream_id = source.stream_id();
        let instrument = source.canonical_instrument();

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
        hub.register_factory(stream_id.clone(), factory, &instrument);

        w.append(Operations::Initialize(lob));
        hub.refresh_merged_for(&instrument);

        Ok((
            symbol.clone(),
            GlobalBookShard {
                book: OrderBook::from(w),
                hub,
                instrument,
                stream_id,
            },
        ))
    }
}

/// Run one multiplexed WebSocket per provider, all feeding `hub`.
pub async fn run_global_book_stream(hub: GlobalBookHub, sources: &[BookSource]) {
    use crate::connectors::stream;
    use futures_util::future::join_all;
    use std::collections::HashMap;

    let mut by_provider: HashMap<Provider, Vec<String>> = HashMap::new();
    for s in sources {
        by_provider
            .entry(s.provider.clone())
            .or_default()
            .push(s.symbol.clone());
    }

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async move {
            let tasks = by_provider.into_iter().map(|(provider, symbols)| {
                let hub = hub.clone();
                async move {
                    let syms: Vec<&str> = symbols.iter().map(String::as_str).collect();
                    match provider {
                        Provider::Binance => {
                            stream::<GlobalBookShard, Depth, _, _>(
                                crate::providers::Binance,
                                (hub, Provider::Binance),
                                &syms,
                            )
                            .await
                        }
                        Provider::BinanceUsdM => {
                            stream::<GlobalBookShard, Depth, _, _>(
                                crate::providers::BinanceUsdM,
                                (hub, Provider::BinanceUsdM),
                                &syms,
                            )
                            .await
                        }
                        Provider::Stub => {
                            stream::<GlobalBookShard, Depth, _, _>(
                                crate::providers::Stub,
                                (hub, Provider::Stub),
                                &syms,
                            )
                            .await
                        }
                        Provider::Other => {
                            warn!(
                                "global book: provider {:?} has no live stream wired yet (scaffold only)",
                                provider.label()
                            );
                        }
                    }
                }
            });
            join_all(tasks).await;
        })
        .await;
}

/// Convenience entry: parse `provider:SYMBOL` list and run until shutdown.
pub async fn stream_global_book(sources: &str) -> Result<(), String> {
    let parsed = parse_book_sources(sources)?;
    let hub = GlobalBookHub::new();
    run_global_book_stream(hub, &parsed).await;
    Ok(())
}

/// Cross-source global book with gRPC / SCALE /metrics on `port` (default 50051).
#[cfg(any(feature = "codec", feature = "grpc"))]
pub async fn stream_global_depth_serve(sources: &str, port: Option<u16>) -> Result<(), String> {
    use crate::servers::{new_registry, start_background};

    let parsed = parse_book_sources(sources)?;
    let registry = new_registry();
    let port = port.unwrap_or(50051);
    let _server = start_background(registry.clone(), port);
    let hub = GlobalBookHub::with_serve_registry(registry);
    run_global_book_stream(hub, &parsed).await;
    Ok(())
}

#[cfg(not(any(feature = "codec", feature = "grpc")))]
pub async fn stream_global_depth_serve(_sources: &str, _port: Option<u16>) -> Result<(), String> {
    Err("global serve requires the `grpc` and/or `codec` feature".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(any(feature = "codec", feature = "grpc"))]
    #[test]
    fn serve_registry_gets_merged_instrument() {
        use crate::servers::new_registry;

        let registry = new_registry();
        let hub = GlobalBookHub::with_serve_registry(registry.clone());
        let (mut w, r) = left_right::new::<LimitOrderBook, Operations>();
        w.append(Operations::Initialize(LimitOrderBook::new()));
        hub.register_factory("binance:BTCUSDT".into(), r.factory(), "BTCUSDT");

        assert!(registry
            .lock()
            .expect("lock")
            .contains_key("BTCUSDT"));
    }

    #[test]
    fn rpi_stream_does_not_pollute_standard_merge_bucket() {
        use lob::PriceAndQuantity;

        let hub = GlobalBookHub::new();

        let mut std_book = LimitOrderBook::new();
        std_book.add_bid(PriceAndQuantity(50_000.0, 2.0));
        std_book.update_id = 10;
        let (mut std_w, std_r) = left_right::new::<LimitOrderBook, Operations>();
        std_w.append(Operations::Initialize(std_book));
        std_w.publish();
        hub.register_factory(
            "binance-usd-m:BTCUSDT".into(),
            std_r.factory(),
            "BTCUSDT",
        );

        let mut rpi_book = LimitOrderBook::new();
        rpi_book.add_bid(PriceAndQuantity(49_000.0, 9.0));
        rpi_book.update_id = 20;
        let (mut rpi_w, rpi_r) = left_right::new::<LimitOrderBook, Operations>();
        rpi_w.append(Operations::Initialize(rpi_book));
        rpi_w.publish();
        hub.register_factory(
            "binance-usd-m:RPI:BTCUSDT".into(),
            rpi_r.factory(),
            "RPI:BTCUSDT",
        );

        hub.refresh_merged_for("BTCUSDT");
        hub.refresh_merged_for("RPI:BTCUSDT");

        let merged_std = hub
            .merged_factory_for("BTCUSDT")
            .unwrap()
            .handle()
            .enter()
            .unwrap()
            .clone();
        let merged_rpi = hub
            .merged_factory_for("RPI:BTCUSDT")
            .unwrap()
            .handle()
            .enter()
            .unwrap()
            .clone();

        assert_eq!(merged_std.update_id, 10);
        assert_eq!(merged_rpi.update_id, 20);
        let std_text = format!("{merged_std}");
        let rpi_text = format!("{merged_rpi}");
        assert!(std_text.contains("50000:2"), "{std_text}");
        assert!(rpi_text.contains("49000:9"), "{rpi_text}");
        assert!(!std_text.contains("49000:9"), "RPI book leaked into BTCUSDT merge");
    }
}
