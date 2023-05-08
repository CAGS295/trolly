use crate::{
    net::streaming::{EventHandler, Message},
    providers::NullResponse,
};
use async_trait::async_trait;
use left_right::{Absorb, WriteHandle};
use lob::{DepthUpdate, LimitOrderBook};
use tracing::{info, trace};

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
        trace!("{}", self);
    }

    fn sync_with(&mut self, first: &Self) {
        self.extend(first);
    }
}

#[async_trait]
impl EventHandler for OrderBook {
    async fn handle_event(&mut self, msg: Message) -> Result<(), ()> {
        let data = msg.into_data();
        let Ok(update)= serde_json::from_slice::<DepthUpdate>(&data).map_err(|e| trace!("Failed to deserialize DepthUpdate {e}")) else {
            let Ok(response) = serde_json::from_slice::<NullResponse>(&data) else{
                return Err(());
            };

            if response.result.is_none() {
                return Ok(());
            }

            return Err(());
        };

        info!(
            "Appending updates [{},{}]",
            update.first_update_id, update.last_update_id
        );
        self.0.append(Operations::Update(update)).publish();
        Ok(())
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
