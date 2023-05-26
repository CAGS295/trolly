use crate::connectors::multiplexor::MonitorMultiplexor;
pub use crate::net::ws_adapter::{connect, disconnect};
use crate::providers::Endpoints;
use crate::signals::Terminate;
use crate::EventHandler;
use futures_util::SinkExt;
use futures_util::StreamExt;
use tokio::net::TcpStream;
pub(crate) use tokio_tungstenite::tungstenite::protocol::Message;
use tokio_tungstenite::MaybeTlsStream;
use tokio_tungstenite::WebSocketStream;
use tracing::debug;
use tracing::{error, info};
use url::Url;

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
        P: Endpoints<Monitorable> + Sync + Clone,
        Context: Sync + Clone,
        Monitorable: 's,
        Handle: EventHandler<'s, Monitorable, Context = Context> + 's,
    {
        let ctrl_c = Terminate::new();

        let mut stream = self.subscribe().await;

        let mut handler = MonitorMultiplexor::<Handle, Monitorable>::build(
            self.provider.clone(),
            self.symbols.as_slice(),
            self.context.clone(),
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
