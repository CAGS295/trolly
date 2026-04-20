use super::handler::{connect, disconnect, EventHandler};
use crate::{providers::Endpoints, signals::Terminate};
use futures_util::{SinkExt, StreamExt};
use std::{collections::HashMap, marker::PhantomData};
use tokio::net::TcpStream;
use tokio_tungstenite::{tungstenite::Message, MaybeTlsStream, WebSocketStream};
use tracing::{debug, error, info};

///Middleware to handle multiple symbols for the same Monitor.
// TODO can we remove Monitorable
pub(crate) struct MonitorMultiplexor<Handle, Monitorable> {
    pub writers: HashMap<String, Handle>,
    _p: PhantomData<Monitorable>,
}

impl<H, M> MonitorMultiplexor<H, M> {
    async fn build<En>(
        provider: En,
        symbols: &[impl AsRef<str>],
        sender: H::Context,
    ) -> Result<Self, H::Error>
    where
        En: Endpoints<M> + Clone + 'static,
        H: EventHandler<M> + 'static,
        H::Context: Clone + 'static,
    {
        let mut writers = HashMap::with_capacity(symbols.len());
        let mut handles = Vec::with_capacity(symbols.len());

        for s in symbols {
            let provider = provider.clone();
            let sender = sender.clone();
            let x = String::from(s.as_ref());
            let f = async move {
                H::build(provider, &[&x], sender)
                    .await
                    .unwrap_or_else(|e| panic!("Building underlying handle for {e:?}",))
            };

            handles.push(tokio::task::spawn_local(f));
        }

        for h in handles {
            if let Ok((id, handle)) = h.await {
                writers.insert(id, handle);
            }
        }

        Ok(Self {
            writers,
            _p: PhantomData,
        })
    }

    pub async fn stream<P, Context>(provider: P, context: Context, symbols: &[&str])
    where
        P: Endpoints<M> + Sync + Clone + 'static,
        Context: Sync + Clone + 'static,
        H: EventHandler<M, Context = Context> + 'static,
    {
        let ctrl_c = Terminate::new();

        let mut stream = Self::subscribe(&provider, symbols).await;

        let mut handler = Self::build(provider, symbols, context).await.unwrap();

        while let (Some(res), false) = (stream.next().await, ctrl_c.is_terminated()) {
            match res {
                Ok(msg) if msg.is_text() => match H::parse_update(msg) {
                    Ok(Some(update)) => {
                        let id = H::to_id(&update);
                        let Some(handler) = handler.writers.get_mut(id) else {
                            error!("missing handler id {id}");
                            continue;
                        };

                        handler
                            .handle_update(update)
                            .inspect_err(|e| error!("{e:?}"))
                            .ok();
                    }
                    Ok(None) => continue,
                    Err(e) => error!("{e:?}"),
                },
                Ok(msg) => {
                    info!("Unhandled Message: {:?}", msg);
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

    async fn subscribe<P>(
        provider: &P,
        symbols: &[&str],
    ) -> WebSocketStream<MaybeTlsStream<TcpStream>>
    where
        P: Endpoints<M> + Sync,
    {
        let mut stream = {
            let url = provider.websocket_url().parse().expect("invalid websocket");
            connect(url).await.expect("stream")
        };

        for msg in provider.ws_subscriptions(symbols.iter()) {
            stream
                .send(Message::Text(msg.into()))
                .await
                .expect("Failed to send subscription message.");
        }
        stream
    }
}
