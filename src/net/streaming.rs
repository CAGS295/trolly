use crate::connectors::MonitorMultiplexor;
pub use crate::net::ws_adapter::{connect, disconnect};
use crate::providers::Endpoints;
use crate::signals::Terminate;
use async_trait::async_trait;
use futures_util::SinkExt;
use futures_util::StreamExt;
use std::fmt::Debug;
use tokio::net::TcpStream;
pub(crate) use tokio_tungstenite::tungstenite::protocol::Message;
use tokio_tungstenite::MaybeTlsStream;
use tokio_tungstenite::WebSocketStream;
use tracing::debug;
use tracing::{error, info};
use url::Url;

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
        provider: &'s En,
        symbols: &'s [String],
        ctx: &'s Self::Context,
    ) -> Result<Self, Self::Error>
    where
        En: Endpoints<Monitorable> + Sync,
        Self: Sized,
        Self::Context: Sync;
}

pub(crate) struct MultiSymbolStream<P, Context> {
    pub symbols: Vec<String>,
    pub provider: P,
    context: Context,
}

impl<P, Context> MultiSymbolStream<P, Context> {
    pub(crate) fn new(symbols: Vec<String>, provider: P, context: Context) -> Self {
        Self {
            symbols,
            provider,
            context,
        }
    }

    pub(crate) async fn stream<'s, Monitorable, Handle>(&'s self)
    where
        P: Endpoints<Monitorable> + Sync,
        Context: Sync,
        Monitorable: 's,
        Handle: EventHandler<'s, Monitorable, Context = Context> + 's,
    {
        let ctrl_c = Terminate::new();

        let mut stream = self.subscribe().await;

        let mut handler = MonitorMultiplexor::<Handle, Monitorable>::build(
            &self.provider,
            self.symbols.as_slice(),
            &self.context,
        )
        .await
        .unwrap();

        while let (Some(res), false) = (stream.next().await, ctrl_c.is_terminated()) {
            match res {
                Ok(msg) if msg.is_text() => {
                    if handler.handle(msg).is_err() {
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

    async fn subscribe<M>(&self) -> WebSocketStream<MaybeTlsStream<TcpStream>>
    where
        P: Endpoints<M> + Sync,
    {
        let mut stream = {
            let url = Url::parse(&self.provider.websocket_url()).expect("invalid websocket");
            connect(url).await.expect("stream")
        };

        stream
            .send(Message::Text(
                self.provider.ws_subscriptions(self.symbols.iter()),
            ))
            .await
            .expect("Failed to Send subscriptions.");
        stream
    }
}
