use crate::{
    net::streaming::{EventHandler, Message},
    providers::{Endpoints, NullResponse},
};
use async_trait::async_trait;
use left_right::{Absorb, ReadHandleFactory, WriteHandle};
use lob::{DepthUpdate, LimitOrderBook};
use tokio::sync::mpsc::UnboundedSender;
use tracing::error;
use tracing::{info, trace, warn};

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
                    self.add_bid(bid.clone());
                }

                for ask in update.asks.iter() {
                    self.add_ask(ask.clone());
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

#[async_trait(?Send)]
impl<'h> EventHandler<'h, Depth> for OrderBook {
    type Error = color_eyre::eyre::Error;
    type Context = UnboundedSender<(String, ReadHandleFactory<LimitOrderBook>)>;
    type Update = DepthUpdate;

    fn parse_update(value: Message) -> Result<Option<Self::Update>, ()> {
        let data = value.into_data();
        let update_result = serde_json::from_slice::<DepthUpdate>(&data)
            .map_err(|e| trace!("Failed to deserialize DepthUpdate: {}", e));

        match update_result {
            Ok(update) => Ok(Some(update)),
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

    fn to_id<'a>(update: &'a DepthUpdate) -> &'a str {
        &update.event.symbol
    }

    fn handle_update(&mut self, update: DepthUpdate) -> Result<(), ()> {
        info!(
            "Appending updates [{},{}]",
            update.first_update_id, update.last_update_id
        );
        self.0.append(Operations::Update(update)).publish();
        Ok(())
    }

    /// Although this takes a symbol slice, it only processes the first element.
    async fn build<E>(
        provider: &'h E,
        symbols: &'h [String],
        sender: &'h UnboundedSender<(String, ReadHandleFactory<LimitOrderBook>)>,
    ) -> Result<Self, Self::Error>
    where
        E: Endpoints<Depth> + Sync,
        Self: Sized,
        Self::Context: Sync,
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
        w.append(Operations::Update(update.clone())).publish();
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
                book.add_ask(i.clone());
            }
            book
        };

        let x = DepthUpdate {
            asks: asks.clone(),
            ..Default::default()
        };
        //Skip the first publish optimization.
        w.publish();
        w.append(Operations::Update(x));
        w.publish();
        w.publish();
        r.enter().map(|guard| assert_eq!(guard.deref(), &book));
    }
}
