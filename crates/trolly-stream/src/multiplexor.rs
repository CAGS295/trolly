use std::{collections::HashMap, marker::PhantomData, sync::atomic::{AtomicBool, Ordering}};

use futures_util::{SinkExt, StreamExt};
use tokio::net::TcpStream;
use tokio_tungstenite::{tungstenite::Message, MaybeTlsStream, WebSocketStream};
use tracing::{debug, error, info};

use crate::endpoints::{StreamEndpoints, VenueEndpoints};
use crate::handler::EventHandler;
use crate::ws_adapter::{connect, disconnect};

/// Middleware to handle multiple symbols for the same monitor.
pub struct MonitorMultiplexor<Handle, Monitorable> {
    pub writers: HashMap<String, Handle>,
    _p: PhantomData<Monitorable>,
}

impl<H, M> MonitorMultiplexor<H, M> {
    /// Build a multiplexor from pre-built per-id handlers (injectable ingress / tests).
    pub fn from_writers(writers: HashMap<String, H>) -> Self {
        Self {
            writers,
            _p: PhantomData,
        }
    }

    async fn build<En>(
        provider: En,
        symbols: &[impl AsRef<str>],
        sender: H::Context,
    ) -> Result<Self, H::Error>
    where
        En: VenueEndpoints + Clone + 'static,
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

    /// Route one websocket `Message` to the per-symbol handler keyed by [`EventHandler::to_id`].
    ///
    /// Injectable ingress: callers can push synthetic or fan-in messages without a socket read.
    pub fn ingest_message(&mut self, msg: Message)
    where
        H: EventHandler<M>,
    {
        match msg {
            msg if msg.is_text() => match H::parse_update(msg) {
                Ok(Some(update)) => {
                    let id = H::to_id(&update);
                    let Some(handler) = self.writers.get_mut(id) else {
                        error!("missing handler id {id}");
                        return;
                    };

                    handler
                        .handle_update(update)
                        .inspect_err(|e| error!("{e:?}"))
                        .ok();
                }
                Ok(None) => {}
                Err(e) => error!("{e:?}"),
            },
            msg => {
                info!("Unhandled Message: {:?}", msg);
            }
        }
    }

    pub async fn stream<P, Context>(
        provider: P,
        context: Context,
        symbols: &[&str],
        shutdown: &AtomicBool,
    ) where
        P: VenueEndpoints + Sync + Clone + 'static,
        Context: Sync + Clone + 'static,
        H: EventHandler<M, Context = Context> + 'static,
    {
        let mut stream = Self::subscribe(&provider, symbols).await;

        let mut handler = Self::build(provider, symbols, context).await.unwrap();

        while !shutdown.load(Ordering::Relaxed) {
            let Some(res) = stream.next().await else {
                break;
            };
            match res {
                Ok(msg) => handler.ingest_message(msg),
                Err(e) => error!("{e}"),
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
        P: StreamEndpoints + Sync,
    {
        let mut stream = {
            let url = provider.websocket_url().parse().expect("invalid websocket");
            connect(url).await.expect("stream")
        };

        for msg in provider.ws_subscriptions(symbols.iter().copied()) {
            stream
                .send(Message::Text(msg.into()))
                .await
                .expect("Failed to send subscription message.");
        }
        stream
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    struct TestUpdate {
        symbol: String,
    }

    struct RecordingHandler {
        hits: Arc<Mutex<Vec<String>>>,
    }

    impl EventHandler<()> for RecordingHandler {
        type Error = std::convert::Infallible;
        type Context = Arc<Mutex<Vec<String>>>;
        type Update = TestUpdate;

        fn parse_update(value: Message) -> Result<Option<Self::Update>, Self::Error> {
            let Message::Text(text) = value else {
                return Ok(None);
            };
            let json: serde_json::Value = serde_json::from_str(&text).unwrap();
            let symbol = json["symbol"].as_str().unwrap().to_string();
            Ok(Some(TestUpdate { symbol }))
        }

        fn to_id(event: &Self::Update) -> &str {
            &event.symbol
        }

        fn handle_update(&mut self, event: Self::Update) -> Result<(), Self::Error> {
            self.hits.lock().unwrap().push(event.symbol);
            Ok(())
        }

        async fn build<En>(
            _provider: En,
            symbols: &[impl AsRef<str>],
            hits: Self::Context,
        ) -> Result<(String, Self), Self::Error> {
            let symbol = symbols.first().unwrap().as_ref().to_string();
            Ok((
                symbol.clone(),
                RecordingHandler { hits },
            ))
        }
    }

    #[test]
    fn ingest_routes_to_correct_symbol_handler() {
        let hits = Arc::new(Mutex::new(Vec::new()));
        let mut writers = HashMap::new();
        for sym in ["BTCUSDT", "ETHUSDT"] {
            writers.insert(
                sym.into(),
                RecordingHandler {
                    hits: hits.clone(),
                },
            );
        }
        let mut hub = MonitorMultiplexor::<RecordingHandler, ()> {
            writers,
            _p: PhantomData,
        };

        hub.ingest_message(Message::Text(
            r#"{"symbol":"BTCUSDT","price":1}"#.into(),
        ));
        hub.ingest_message(Message::Text(
            r#"{"symbol":"ETHUSDT","price":2}"#.into(),
        ));
        hub.ingest_message(Message::Text(
            r#"{"symbol":"BTCUSDT","price":3}"#.into(),
        ));

        let recorded = hits.lock().unwrap().clone();
        assert_eq!(recorded, vec!["BTCUSDT", "ETHUSDT", "BTCUSDT"]);
    }
}
