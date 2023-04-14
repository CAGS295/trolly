use std::ops::Deref;

pub use crate::net::ws_adapter::{connect, disconnect};
use crate::signals::Terminate;
use async_trait::async_trait;
use futures_util::StreamExt;
use log::debug;
pub(crate) use tokio_tungstenite::tungstenite::{protocol::Message, Error};
use url::Url;

#[async_trait]
pub(crate) trait EventHandler {
    /// Use the Result to break out of the handler loop;
    async fn handle_event(&self, event: Result<Message, Error>) -> Result<(), ()>;
}

#[async_trait]
/// We want to be able to use the handle without yielding ownership.
impl<T: EventHandler + Sync> EventHandler for &T {
    async fn handle_event(&self, event: Result<Message, Error>) -> Result<(), ()> {
        self.deref().handle_event(event).await
    }
}

pub struct SimpleHandler<'a> {
    pub(crate) url: &'a str,
}

impl<'a> SimpleHandler<'a> {
    pub(crate) async fn stream<T: EventHandler>(&self, handler: T) {
        let ctrl_c = Terminate::new();

        let url = Url::parse(self.url).expect("invalid websocket");
        let mut stream = connect(url).await.expect("stream");
        while let (Some(x), false) = (stream.next().await, ctrl_c.is_terminated()) {
            if handler.handle_event(x).await.is_err() {
                break;
            }
        }

        if let Err(e) = disconnect(&mut stream).await {
            debug!("Failed to disconnect {e}: {stream:?}");
        };
    }
}
