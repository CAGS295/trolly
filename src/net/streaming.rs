use crate::connectors::multiplexor::MonitorMultiplexor;
pub use crate::net::ws_adapter::{connect, disconnect};
use crate::providers::Endpoints;
use crate::signals::Terminate;
use crate::EventHandler;
use futures_util::SinkExt;
use futures_util::StreamExt;
use tokio::net::TcpStream;
pub use tokio_tungstenite::tungstenite::protocol::Message;
use tokio_tungstenite::MaybeTlsStream;
use tokio_tungstenite::WebSocketStream;
use tracing::debug;
use tracing::{error, info};

pub struct MultiSymbolStream;

impl MultiSymbolStream {
    pub async fn stream<Monitorable, Handle, P, Context>(
        provider: P,
        context: Context,
        symbols: impl AsRef<[String]>,
    ) where
        P: Endpoints<Monitorable> + Sync + Clone + 'static,
        Context: Sync + Clone + 'static,
        Handle: EventHandler<Monitorable, Context = Context> + 'static,
    {
        let ctrl_c = Terminate::new();

        let mut stream = Self::subscribe(&provider, symbols.as_ref()).await;

        let mut handler =
            MonitorMultiplexor::<Handle, Monitorable>::build(provider, symbols.as_ref(), context)
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

    async fn subscribe<P, M>(
        provider: &P,
        symbols: &[String],
    ) -> WebSocketStream<MaybeTlsStream<TcpStream>>
    where
        P: Endpoints<M> + Sync,
    {
        let mut stream = {
            let url = provider.websocket_url().parse().expect("invalid websocket");
            connect(url).await.expect("stream")
        };

        stream
            .send(Message::Text(provider.ws_subscriptions(symbols.iter())))
            .await
            .expect("Failed to Send subscriptions.");
        stream
    }
}
