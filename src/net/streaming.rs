pub use crate::net::ws_adapter::{connect, disconnect};
use crate::providers::Endpoints;
use crate::signals::Terminate;
use async_trait::async_trait;
use futures_util::Future;
use futures_util::SinkExt;
use futures_util::StreamExt;
use std::error::Error;
pub(crate) use tokio_tungstenite::tungstenite::protocol::Message;
use tracing::debug;
use tracing::{error, info};
use url::Url;

#[async_trait]
pub(crate) trait EventHandler {
    /// Use the Result to break out of the handler loop;
    async fn handle_event(&mut self, event: Message) -> Result<(), ()>;
}

pub(crate) struct SimpleStream<'a> {
    pub symbols: &'a Vec<String>,
}

impl<'a> SimpleStream<'a> {
    pub(crate) async fn stream<H, M, E, Output, F, En>(&self, handler_builder: F, endpoints: &'a En)
    where
        H: EventHandler,
        E: Error,
        Output: Future<Output = Result<H, E>>,
        En: Endpoints<M>,
        F: FnOnce(&'a En, &'a Vec<String>) -> Output,
    {
        let ctrl_c = Terminate::new();

        let url = Url::parse(&En::websocket_url()).expect("invalid websocket");
        let mut stream = connect(url).await.expect("stream");
        //subscribe
        stream
            .send(Message::Text(
                endpoints.ws_subscriptions(self.symbols.iter()),
            ))
            .await
            .expect("Failed to Send subscriptions.");

        //route
        let mut handler = handler_builder(&endpoints, self.symbols).await.unwrap();
        while let (Some(res), false) = (stream.next().await, ctrl_c.is_terminated()) {
            match res {
                Ok(msg) if msg.is_text() => {
                    if handler.handle_event(msg).await.is_err() {
                        break;
                    }
                }
                Ok(msg) => {
                    info!("Unhandled Message: {msg}");
                }
                Err(e) => {
                    error!("{e}");
                }
            }
        }

        if let Err(e) = disconnect(&mut stream).await {
            debug!("Failed to disconnect {e}: {stream:?}");
        };
    }
}
