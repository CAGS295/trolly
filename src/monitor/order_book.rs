use crate::net::streaming::{Error, EventHandler, Message};
use async_trait::async_trait;
use lob::{DepthUpdate, LimitOrderBook};
use tracing::{debug, error, info};

pub struct OrderBook(LimitOrderBook);

impl From<LimitOrderBook> for OrderBook {
    fn from(value: LimitOrderBook) -> Self {
        Self(value)
    }
}

#[async_trait]
impl EventHandler for OrderBook {
    async fn handle_event(&mut self, event: Result<Message, Error>) -> Result<(), ()> {
        match event {
            Ok(message) => {
                let mut update: DepthUpdate = serde_json::from_slice(&message.into_data()).unwrap();

                debug!("DepthUpdate : {update:?}");

                for bid in update.bids.drain(..) {
                    self.0.add_bid(bid);
                }

                for ask in update.asks.drain(..) {
                    self.0.add_ask(ask);
                }

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
