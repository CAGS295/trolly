use crate::net::streaming::{EventHandler, Message};
use async_trait::async_trait;
use lob::{DepthUpdate, LimitOrderBook};
use std::error::Error;
use tracing::{debug, error, info};

pub struct OrderBook(LimitOrderBook);

impl From<LimitOrderBook> for OrderBook {
    fn from(value: LimitOrderBook) -> Self {
        Self(value)
    }
}

#[async_trait]
impl EventHandler for OrderBook {
    async fn handle_event(&mut self, event: Result<Message, impl Error + Send>) -> Result<(), ()> {
        match event {
            Ok(message) => {
                let mut update: DepthUpdate = serde_json::from_slice(&message.into_data()).unwrap();

                // immediate transition with update stream.
                if self.0.update_id + 1 < update.first_update_id
                    || update.last_update_id + 1 < self.0.update_id
                {
                    info!(
                        "Skipping stale updates stamp:{} [{},{}]",
                        update.event.time, update.first_update_id, update.last_update_id
                    );
                    return Ok(());
                }

                debug!("DepthUpdate : {update:?}");

                for bid in update.bids.drain(..) {
                    self.0.add_bid(bid);
                }

                for ask in update.asks.drain(..) {
                    self.0.add_ask(ask);
                }

                self.0.update_id = update.last_update_id;

                info!("{:?}", self.0);

                Ok(())
            }
            Err(e) => {
                error!("{e}");
                Err(())
            }
        }
    }
}
