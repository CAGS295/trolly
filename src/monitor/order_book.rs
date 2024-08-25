use crate::{
    net::streaming::Message,
    providers::{Endpoints, NullResponse},
    EventHandler,
};
use left_right::{Absorb, ReadHandleFactory, WriteHandle};
use lob::{DepthUpdate, LimitOrderBook};
use tokio::sync::mpsc::UnboundedSender;
use tracing::error;
use tracing::{info, instrument, trace, warn};

use super::{depth::DepthHandler, Depth};

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
                if self.update_id + 1 < update.first_update_id
                    || update.last_update_id + 1 < self.update_id
                {
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

impl<T> EventHandler<Depth> for T
where
    T: DepthHandler,
{
    type Error = <T as DepthHandler>::Error;
    type Context = <T as DepthHandler>::Context;
    type Update = DepthUpdate;

    #[instrument(skip(value), fields(pair))]
    fn parse_update(value: Message) -> Result<Option<Self::Update>, ()> {
        let data = value.into_data();
        let update_result = serde_json::from_slice::<DepthUpdate>(&data)
            .map_err(|e| trace!("Failed to deserialize DepthUpdate: {}", e));

        match update_result {
            Ok(update) => {
                tracing::Span::current().record("pair", Self::to_id(&update));
                Ok(Some(update))
            }
            Err(_) => {
                let response_result = serde_json::from_slice::<NullResponse>(&data)
                    .map(|response| response.result.is_none());

                match response_result {
                    Ok(true) => Ok(None),
                    _ => Err(()),
                }
            }
        }
    }

    fn to_id(update: &DepthUpdate) -> &str {
        &update.event.symbol
    }

    #[instrument(skip_all, fields(pair))]
    fn handle_update(&mut self, update: DepthUpdate) -> Result<(), ()> {
        tracing::Span::current().record("pair", Self::to_id(&update));
        info!("[{},{}]", update.first_update_id, update.last_update_id);

        DepthHandler::handle_update(self, update)
    }

    /// Although this takes a symbol slice, it only processes the first element.
    async fn build<En>(
        provider: En,
        symbols: &[String],
        sender: Self::Context,
    ) -> Result<Self, Self::Error>
    where
        En: Endpoints<Depth>,
    {
        <T as DepthHandler>::build(provider, symbols, sender).await
    }
}

impl DepthHandler for OrderBook {
    type Error = color_eyre::eyre::Error;
    type Context = UnboundedSender<(String, ReadHandleFactory<LimitOrderBook>)>;

    fn handle_update(&mut self, update: DepthUpdate) -> Result<(), ()> {
        self.0.append(Operations::Update(update)).publish();
        Ok(())
    }

    async fn build<En>(
        provider: En,
        symbols: &[String],
        sender: Self::Context,
    ) -> Result<Self, Self::Error>
    where
        En: Endpoints<Depth>,
        Self: Sized,
    {
        let symbol = symbols.first().expect("required");
        //query
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

        if let Err(e) = sender.send((symbol.to_string(), r.factory())) {
            warn!("Failed to send {symbol} handler to the RPC server. {e}");
        }

        w.append(Operations::Initialize(lob));
        Ok(OrderBook::from(w))
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use lob::Asks;
    use lob::PriceAndQuantity;
    use std::ops::Deref;

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
        assert_eq!(guard.deref(),&book);
    }
}
