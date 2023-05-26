pub use crate::net::ws_adapter::{connect, disconnect};
use crate::providers::Endpoints;
use async_trait::async_trait;
use std::fmt::Debug;
pub(crate) use tokio_tungstenite::tungstenite::protocol::Message;

#[async_trait(?Send)]
pub(crate) trait EventHandler<'s, Monitorable> {
    type Error: Debug;
    type Context;
    type Update;

    /// returning Ok(None) is not a fatal failure and the scheduler should skip the message.
    fn parse_update(value: Message) -> Result<Option<Self::Update>, ()>;

    /// A handler shoud be identifiable from a Self::Update.
    /// This is necessary to route updates to their handler when processing multiple subscriptions from the same source.
    fn to_id<'a>(event: &'a Self::Update) -> &'a str;

    /// Use the Result to break out of the handler loop;
    /// Handle a raw message.
    fn handle(&mut self, msg: Message) -> Result<(), ()> {
        let Some(update) = Self::parse_update(msg)? else{
            //Skip if not a relevant update.
            return Ok(());
        };

        self.handle_update(update)
    }

    /// Handle a parsed Update.
    /// Use the Result to break out of the handler loop;
    fn handle_update(&mut self, event: Self::Update) -> Result<(), ()>;

    /// don't take a writer, take a tx and send the readerfactory with the symbol to the gRPC server.
    /// return context.
    async fn build<En>(
        provider: En,
        symbols: &'s [String],
        ctx: Self::Context,
    ) -> Result<Self, Self::Error>
    where
        En: Endpoints<Monitorable> + Sync + Clone,
        Self: Sized,
        Self::Context: Sync + Clone;
}
