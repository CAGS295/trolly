//! Depth [`crate::EventHandler`] that prints each update to stdout (no REST snapshot, no server).

use crate::{
    providers::Endpoints,
    EventHandler,
};
use lob::DepthUpdate;
use tokio_tungstenite::tungstenite::Message;
use tracing::{instrument, warn};

use super::{depth_parse, Depth};

/// Prints parsed depth updates; does not maintain an order book or expose gRPC / SCALE.
pub struct EchoDepth;

impl EventHandler<Depth> for EchoDepth {
    type Error = color_eyre::eyre::Error;
    type Context = ();
    type Update = DepthUpdate;

    fn parse_update(value: Message) -> Result<Option<Self::Update>, Self::Error> {
        depth_parse::parse_depth_message(value)
    }

    fn to_id(update: &DepthUpdate) -> &str {
        &update.event.symbol
    }

    #[instrument(skip_all, fields(pair))]
    fn handle_update(&mut self, update: DepthUpdate) -> Result<(), Self::Error> {
        tracing::Span::current().record("pair", Self::to_id(&update));
        println!(
            "{} first={} last={} prev={:?} bids={} asks={}",
            update.event.symbol,
            update.first_update_id,
            update.last_update_id,
            update.previous_update_id,
            update.bids.len(),
            update.asks.len(),
        );
        Ok(())
    }

    async fn build<En>(
        _provider: En,
        symbols: &[impl AsRef<str>],
        (): Self::Context,
    ) -> Result<(String, Self), Self::Error>
    where
        En: Endpoints<Depth>,
    {
        assert_eq!(symbols.len(), 1, "echo handler is built per symbol");
        let symbol = symbols
            .first()
            .expect("one symbol")
            .as_ref()
            .to_uppercase();
        warn!("echo mode: no REST snapshot for {symbol}; printing deltas only");
        Ok((symbol, EchoDepth))
    }
}
