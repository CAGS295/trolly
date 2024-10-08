pub use crate::net::ws_adapter::{connect, disconnect};
use crate::providers::Endpoints;
use std::{fmt::Debug, future::Future};
pub(crate) use tokio_tungstenite::tungstenite::protocol::Message;

pub trait EventHandler<Monitorable> {
    type Error: Debug;
    type Context;
    type Update;

    /// returning Ok(None) is not a fatal failure and the scheduler should skip the message.
    fn parse_update(value: Message) -> Result<Option<Self::Update>, Self::Error>;

    /// A handler shoud be identifiable from a Self::Update.
    /// This is necessary to route updates to their handler when processing multiple subscriptions from the same source.
    fn to_id(event: &Self::Update) -> &str;

    /// Use the Result to break out of the handler loop;
    /// Handle a raw message.
    fn handle(&mut self, msg: Message) -> Result<(), Self::Error> {
        let Some(update) = Self::parse_update(msg)? else {
            //Skip if not a relevant update.
            return Ok(());
        };

        self.handle_update(update)
    }

    /// Handle a parsed Update.
    /// Use the Result to break out of the handler loop;
    fn handle_update(&mut self, event: Self::Update) -> Result<(), Self::Error>;

    /// don't take a writer, take a tx and send the readerfactory with the symbol to the gRPC server.
    /// return context.
    fn build<En>(
        provider: En,
        symbols: &[String],
        ctx: Self::Context,
    ) -> impl Future<Output = Result<Self, Self::Error>>
    where
        En: Endpoints<Monitorable> + Clone + 'static,
        Self: Sized,
        Self::Context: Clone + 'static;
}
