use crate::{providers::Endpoints, EventHandler};
use left_right::{Absorb, ReadHandleFactory, WriteHandle};
use lob::{DepthUpdate, LimitOrderBook};
use tokio::sync::mpsc::UnboundedSender;
use tokio_tungstenite::tungstenite::Message;
use tracing::error;
use tracing::{info, instrument, trace, warn};

use super::depth_parse;
use super::Depth;

pub struct OrderBook(WriteHandle<LimitOrderBook, Operations>);

impl From<WriteHandle<LimitOrderBook, Operations>> for OrderBook {
    fn from(value: WriteHandle<LimitOrderBook, Operations>) -> Self {
        Self(value)
    }
}
pub(crate) enum Operations {
    Update(DepthUpdate),
    Initialize(LimitOrderBook),
}

impl Absorb<Operations> for LimitOrderBook {
    fn absorb_first(&mut self, operation: &mut Operations, other: &Self) {
        match operation {
            Operations::Update(update) => {
                // immediate transition with update stream.
                if update.skip_update(self.update_id) {
                    info!(
                        "Skipping stale updates stamp:{} [{},{}]",
                        update.event.time, update.first_update_id, update.last_update_id
                    );
                    return;
                }

                trace!("Absorb DepthUpdate: {update}");

                for bid in update.bids.iter() {
                    self.add_bid(*bid);
                }

                for ask in update.asks.iter() {
                    self.add_ask(*ask);
                }

                self.update_id = update.last_update_id;
            }
            Operations::Initialize(book) => {
                //Should only be used to populate the initial structure, i.e. loading from deserialization.
                assert!(*self == LimitOrderBook::default());
                assert!(*other == LimitOrderBook::default());
                self.extend(book);
            }
        }
        trace!("Book: {}", self);
    }

    fn sync_with(&mut self, first: &Self) {
        self.extend(first);
    }
}

impl EventHandler<Depth> for OrderBook {
    type Error = color_eyre::eyre::Error;
    type Context = UnboundedSender<(String, ReadHandleFactory<LimitOrderBook>)>;
    type Update = DepthUpdate;

    #[instrument(skip(value), fields(pair))]
    fn parse_update(value: Message) -> Result<Option<Self::Update>, Self::Error> {
        let out = depth_parse::parse_depth_message(value)?;
        if let Some(ref u) = out {
            tracing::Span::current().record("pair", Self::to_id(u));
        }
        Ok(out)
    }

    fn to_id(update: &DepthUpdate) -> &str {
        &update.event.symbol
    }

    #[instrument(skip_all, fields(pair))]
    fn handle_update(&mut self, update: DepthUpdate) -> Result<(), Self::Error> {
        tracing::Span::current().record("pair", Self::to_id(&update));
        info!("[{},{}]", update.first_update_id, update.last_update_id);

        self.0.append(Operations::Update(update)).publish();
        Ok(())
    }

    /// Although this takes a symbol slice, it only processes the first element.
    async fn build<En>(
        provider: En,
        symbols: &[impl AsRef<str>],
        sender: Self::Context,
    ) -> Result<(String, Self), Self::Error>
    where
        En: Endpoints<Depth>,
    {
        assert_eq!(symbols.len(), 1);
        let symbol = symbols.first().expect("exactly one");

        let response: reqwest::Response = {
            let url = provider.rest_api_url(symbol);
            reqwest::get(url).await
        }?;
        if let Err(e) = response.error_for_status_ref() {
            error!("Query failed:{e} {}", response.text().await?);
            return Err(e.into());
        };
        let lob: LimitOrderBook = response.json().await?;

        let (mut w, r) = left_right::new();

        let symbol = symbol.as_ref().to_string();
        if let Err(e) = sender.send((symbol.clone(), r.factory())) {
            warn!(
                "Failed to send {} handler to the RPC server. {e}",
                provider.rest_api_url(&symbol)
            );
        }

        w.append(Operations::Initialize(lob));
        Ok((symbol, OrderBook::from(w)))
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::EventHandler;
    use lob::Asks;
    use lob::PriceAndQuantity;
    use std::ops::Deref;
    use tokio_tungstenite::tungstenite::Message;

    #[test]
    fn update_id_not_read_from_other() {
        let mut update = DepthUpdate {
            last_update_id: 2,
            ..Default::default()
        };
        let (mut w, r) = left_right::new::<LimitOrderBook, _>();
        w.publish();
        w.append(Operations::Update(update.clone()));
        update.first_update_id = 3;
        update.last_update_id = 5;
        w.append(Operations::Update(update)).publish();
        let id = r.enter().map(|guard| guard.update_id).unwrap();
        assert_eq!(id, 5);
    }

    #[test]
    fn left_right_does_not_drift_appart() {
        let asks: Asks = vec![PriceAndQuantity(1.0, 1.0)].into();
        let (mut w, r) = left_right::new::<LimitOrderBook, Operations>();
        let book = {
            let mut book = LimitOrderBook::new();
            for i in asks.iter() {
                book.add_ask(*i);
            }
            book
        };

        let x = DepthUpdate {
            asks,
            ..Default::default()
        };
        //Skip the first publish optimization.
        w.publish();
        w.append(Operations::Update(x));
        w.publish();
        w.publish();
        let guard = r.enter().unwrap();
        assert_eq!(guard.deref(), &book);
    }

    #[test]
    fn parse_update_rpi_envelope_prepends_prefix() {
        let json = include_str!("../../tests/fixtures/binance_usd_m_rpi_depth_envelope.json");
        let msg = Message::Text(json.into());
        let update = OrderBook::parse_update(msg).unwrap().unwrap();

        assert_eq!(OrderBook::to_id(&update), "RPI:BTCUSDT");
        assert_eq!(update.event.symbol, "RPI:BTCUSDT");
        assert_eq!(update.first_update_id, 4541941613);
        assert_eq!(update.bids.len(), 2);
        assert_eq!(update.asks.len(), 1);
    }

    #[test]
    fn parse_update_standard_envelope_preserves_symbol() {
        let json = include_str!("../../tests/fixtures/binance_usd_m_depth_envelope.json");
        let msg = Message::Text(json.into());
        let update = OrderBook::parse_update(msg).unwrap().unwrap();

        assert_eq!(OrderBook::to_id(&update), "BTCUSDT");
        assert_eq!(update.event.symbol, "BTCUSDT");
        assert_eq!(update.first_update_id, 4541941613);
    }

    #[test]
    fn parse_update_raw_depth_still_works() {
        let json = include_str!("../../tests/fixtures/binance_usd_m_depth_update.json");
        let msg = Message::Text(json.into());
        let update = OrderBook::parse_update(msg).unwrap().unwrap();

        assert_eq!(OrderBook::to_id(&update), "BTCUSDT");
        assert_eq!(update.previous_update_id, Some(4541941610));
    }

    #[test]
    fn parse_update_subscription_ack_returns_none() {
        let json = r#"{"result":null,"id":1}"#;
        let msg = Message::Text(json.into());
        let result = OrderBook::parse_update(msg).unwrap();

        assert!(result.is_none());
    }

    #[test]
    fn to_id_routes_rpi_and_standard_to_different_keys() {
        use lob::limit_order_book::event::Event;

        let standard = DepthUpdate {
            event: Event {
                symbol: "BTCUSDT".into(),
                ..Default::default()
            },
            ..Default::default()
        };
        let rpi = DepthUpdate {
            event: Event {
                symbol: "RPI:BTCUSDT".into(),
                ..Default::default()
            },
            ..Default::default()
        };

        assert_ne!(OrderBook::to_id(&standard), OrderBook::to_id(&rpi));
        assert_eq!(OrderBook::to_id(&standard), "BTCUSDT");
        assert_eq!(OrderBook::to_id(&rpi), "RPI:BTCUSDT");
    }

    /// If the server sends unwrapped (raw) payloads for an rpiDepth stream,
    /// parse_update cannot distinguish it from a standard depth update.
    /// The RPI handler would never be reached — the update routes to the
    /// standard "BTCUSDT" handler instead of "RPI:BTCUSDT".
    ///
    /// This test documents that the envelope is *required* for correct routing
    /// when both streams coexist on the same connection.
    #[test]
    fn raw_rpi_payload_misroutes_without_envelope() {
        let raw_json = include_str!("../../tests/fixtures/binance_usd_m_depth_update.json");
        let msg = Message::Text(raw_json.into());
        let update = OrderBook::parse_update(msg).unwrap().unwrap();

        // Without the envelope, the symbol is always the raw exchange symbol.
        // An RPI handler keyed as "RPI:BTCUSDT" would never match this.
        assert_eq!(OrderBook::to_id(&update), "BTCUSDT");
        assert_ne!(OrderBook::to_id(&update), "RPI:BTCUSDT");
    }
}
