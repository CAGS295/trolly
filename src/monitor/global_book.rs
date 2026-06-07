//! Cross-source **global order book**: one live book per [`BookSource`], merged by
//! [`BookSource::canonical_instrument`] (e.g. `binance:BTCUSDT` + `binance-usd-m:BTCUSDT` → `BTCUSDT`).
//!
//! Intra-provider overlays (Binance USDM `@depth` vs `@rpiDepth`) are **not** folded into the
//! canonical instrument unless you subscribe both under the same symbol name; RPI remains a
//! venue-specific stream id (`binance-usd-m:RPI:BTCUSDT`).

use std::collections::HashMap;
use std::ops::Deref;
use std::sync::{Arc, Mutex};

use crate::{providers::Endpoints, EventHandler};

use super::{
    order_book::{Operations, OrderBook},
    Depth, Provider,
};
use left_right::{Absorb, ReadHandle, ReadHandleFactory, WriteHandle};
use lob::{DepthUpdate, LimitOrderBook};
use tokio_tungstenite::tungstenite::Message;
use tracing::{instrument, warn};

/// One depth feed: exchange provider + subscription symbol (may include venue-specific prefixes).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct BookSource {
    pub provider: Provider,
    pub symbol: String,
}

impl BookSource {
    pub fn new(provider: Provider, symbol: impl Into<String>) -> Self {
        Self {
            provider,
            symbol: symbol.into().trim().to_uppercase(),
        }
    }

    /// Unique routing / factory key (`binance:BTCUSDT`, `binance-usd-m:RPI:BTCUSDC`, …).
    pub fn stream_id(&self) -> String {
        format!("{}:{}", self.provider.label(), self.symbol)
    }

    /// Instrument identity for cross-source merge (uppercase symbol only).
    pub fn canonical_instrument(&self) -> String {
        self.symbol.clone()
    }

    /// Parse `provider:SYMBOL` (e.g. `binance:BTCUSDT`, `binance-usd-m:RPI:BTCUSDC`).
    pub fn parse(raw: &str) -> Result<Self, String> {
        let raw = raw.trim();
        let (provider_label, symbol) = raw
            .split_once(':')
            .ok_or_else(|| format!("expected provider:SYMBOL, got {raw:?}"))?;
        let provider = Provider::from_label(provider_label)
            .ok_or_else(|| format!("unknown provider {provider_label:?}"))?;
        if symbol.trim().is_empty() {
            return Err(format!("missing symbol in {raw:?}"));
        }
        Ok(Self::new(provider, symbol))
    }
}

impl Provider {
    pub fn label(self) -> &'static str {
        match self {
            Provider::Binance => "binance",
            Provider::BinanceUsdM => "binance-usd-m",
            Provider::Other => "other",
        }
    }

    pub fn from_label(label: &str) -> Option<Self> {
        match label.trim().to_ascii_lowercase().as_str() {
            "binance" => Some(Provider::Binance),
            "binance-usd-m" | "binance_usd_m" | "binanceusdm" => Some(Provider::BinanceUsdM),
            "other" => Some(Provider::Other),
            _ => None,
        }
    }

    /// Whether `label` is a known depth venue for `--sources provider:SYMBOL`.
    pub fn is_registered_label(label: &str) -> bool {
        Self::from_label(label).is_some()
    }
}

/// Parse a comma-separated list of `provider:SYMBOL` entries.
pub fn parse_book_sources(list: &str) -> Result<Vec<BookSource>, String> {
    let mut out = Vec::new();
    for part in list.split(',').map(str::trim).filter(|s| !s.is_empty()) {
        out.push(BookSource::parse(part)?);
    }
    if out.is_empty() {
        return Err("at least one book source is required".into());
    }
    Ok(out)
}

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
                if let Some(guard) = factory.handle().enter() {
                    merged.merge_into(guard.deref());
                }
            }
            merged
        };

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
        En: Endpoints<Depth>,
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
    use crate::connectors::multiplexor::MonitorMultiplexor;
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
                            MonitorMultiplexor::<GlobalBookShard, Depth>::stream::<_, _>(
                                crate::providers::Binance,
                                (hub, Provider::Binance),
                                &syms,
                            )
                            .await
                        }
                        Provider::BinanceUsdM => {
                            MonitorMultiplexor::<GlobalBookShard, Depth>::stream::<_, _>(
                                crate::providers::BinanceUsdM,
                                (hub, Provider::BinanceUsdM),
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

    #[test]
    fn parse_book_source_spot() {
        let s = BookSource::parse("binance:btcusdt").unwrap();
        assert_eq!(s.provider, Provider::Binance);
        assert_eq!(s.symbol, "BTCUSDT");
        assert_eq!(s.stream_id(), "binance:BTCUSDT");
        assert_eq!(s.canonical_instrument(), "BTCUSDT");
    }

    #[test]
    fn parse_book_source_usdm_rpi() {
        let s = BookSource::parse("binance-usd-m:RPI:BTCUSDC").unwrap();
        assert_eq!(s.stream_id(), "binance-usd-m:RPI:BTCUSDC");
        assert_eq!(s.canonical_instrument(), "RPI:BTCUSDC");
    }

    #[test]
    fn parse_book_sources_list() {
        let v = parse_book_sources("binance:BTCUSDT,binance-usd-m:BTCUSDT").unwrap();
        assert_eq!(v.len(), 2);
        assert_eq!(v[0].canonical_instrument(), "BTCUSDT");
        assert_eq!(v[1].canonical_instrument(), "BTCUSDT");
    }

    #[test]
    fn parse_book_source_other_venue() {
        let s = BookSource::parse("other:ETHUSDT").unwrap();
        assert_eq!(s.provider, Provider::Other);
        assert_eq!(s.symbol, "ETHUSDT");
        assert_eq!(s.stream_id(), "other:ETHUSDT");
        assert!(Provider::is_registered_label("other"));
    }

    #[test]
    fn parse_book_sources_three_venues() {
        let v = parse_book_sources("binance:BTCUSDT,binance-usd-m:BTCUSDT,other:BTCUSDT").unwrap();
        assert_eq!(v.len(), 3);
        assert_eq!(v[0].provider, Provider::Binance);
        assert_eq!(v[1].provider, Provider::BinanceUsdM);
        assert_eq!(v[2].provider, Provider::Other);
        assert_eq!(v[0].stream_id(), "binance:BTCUSDT");
        assert_eq!(v[1].stream_id(), "binance-usd-m:BTCUSDT");
        assert_eq!(v[2].stream_id(), "other:BTCUSDT");
        for s in &v {
            assert_eq!(s.canonical_instrument(), "BTCUSDT");
        }
    }

    #[test]
    fn refresh_merged_for_aggregates_cross_source_books() {
        use crate::monitor::order_book::Operations;

        let hub = GlobalBookHub::new();
        let spot: LimitOrderBook = serde_json::from_str(
            r#"{"lastUpdateId":1,"bids":[["50000.0","1.0"]],"asks":[["50001.0","2.0"]]}"#,
        )
        .unwrap();
        let usdm: LimitOrderBook = serde_json::from_str(
            r#"{"lastUpdateId":2,"bids":[["50000.0","0.5"]],"asks":[["50002.0","1.0"]]}"#,
        )
        .unwrap();

        let (mut w_spot, r_spot) = left_right::new::<LimitOrderBook, Operations>();
        w_spot.append(Operations::Initialize(spot));
        w_spot.publish();
        hub.register_factory("binance:BTCUSDT".into(), r_spot.factory(), "BTCUSDT");

        let (mut w_usdm, r_usdm) = left_right::new::<LimitOrderBook, Operations>();
        w_usdm.append(Operations::Initialize(usdm));
        w_usdm.publish();
        hub.register_factory("binance-usd-m:BTCUSDT".into(), r_usdm.factory(), "BTCUSDT");

        hub.refresh_merged_for("BTCUSDT");

        let factory = hub.merged_factory_for("BTCUSDT").expect("merged factory");
        let handle = factory.handle();
        let merged = handle.enter().expect("merged snapshot");
        assert_eq!(merged.update_id, 2);
        let text = format!("{}", merged.deref());
        assert!(text.contains("50000:1.5"), "{text}");
        assert!(text.contains("50001"));
        assert!(text.contains("50002"));
    }

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
}
