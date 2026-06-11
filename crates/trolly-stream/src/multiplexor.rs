use crate::endpoints::Endpoints;
use crate::handler::{connect, disconnect, EventHandler};
use futures_util::{SinkExt, StreamExt};
use std::{collections::HashMap, marker::PhantomData};
use tokio::net::TcpStream;
use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender};
use tokio_tungstenite::{tungstenite::Message, MaybeTlsStream, WebSocketStream};
use tracing::{debug, error, info};

/// Outcome of routing a single websocket frame through the multiplexor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RouteOutcome {
    Handled,
    Skipped,
    MissingHandler,
    ParseError,
    Unhandled,
}

/// Injectable ingress for fanning synthetic or user-data websocket frames into the router.
#[derive(Clone, Debug)]
pub struct MessageIngress {
    tx: UnboundedSender<Message>,
}

impl MessageIngress {
    pub fn push(&self, message: Message) -> Result<(), mpsc::error::SendError<Message>> {
        self.tx.send(message)
    }
}

/// Pair of inject sender and receiver consumed by [`MonitorMultiplexor::stream_until`].
pub fn message_ingress() -> (MessageIngress, UnboundedReceiver<Message>) {
    let (tx, rx) = mpsc::unbounded_channel();
    (MessageIngress { tx }, rx)
}

/// Middleware to handle multiple symbols for the same monitor.
pub struct MonitorMultiplexor<Handle, Monitorable> {
    pub writers: HashMap<String, Handle>,
    _p: PhantomData<Monitorable>,
}

impl<H, M> MonitorMultiplexor<H, M> {
    pub async fn build<En>(
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

    /// Route one websocket frame through parse → `to_id` → `handle_update`.
    pub fn route_message(&mut self, msg: Message) -> RouteOutcome
    where
        H: EventHandler<M>,
    {
        if msg.is_text() {
            match H::parse_update(msg) {
                Ok(Some(update)) => {
                    let id = H::to_id(&update);
                    let Some(handler) = self.writers.get_mut(id) else {
                        error!("missing handler id {id}");
                        return RouteOutcome::MissingHandler;
                    };

                    match handler.handle_update(update) {
                        Ok(()) => RouteOutcome::Handled,
                        Err(e) => {
                            error!("{e:?}");
                            RouteOutcome::ParseError
                        }
                    }
                }
                Ok(None) => RouteOutcome::Skipped,
                Err(e) => {
                    error!("{e:?}");
                    RouteOutcome::ParseError
                }
            }
        } else {
            info!("Unhandled Message: {:?}", msg);
            RouteOutcome::Unhandled
        }
    }

    pub async fn stream<P, Context>(provider: P, context: Context, symbols: &[&str])
    where
        P: Endpoints<M> + Sync + Clone + 'static,
        Context: Sync + Clone + 'static,
        H: EventHandler<M, Context = Context> + 'static,
    {
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::sync::Arc;

        let shutdown = Arc::new(AtomicBool::new(false));
        let flag = shutdown.clone();
        tokio::spawn(async move {
            if tokio::signal::ctrl_c().await.is_ok() {
                flag.store(true, Ordering::Relaxed);
                println!("\r received Ctrl+C! terminating");
            }
        });

        Self::stream_until(
            provider,
            context,
            symbols,
            || shutdown.load(Ordering::Relaxed),
            None,
        )
        .await
    }

    pub async fn stream_until<P, Context>(
        provider: P,
        context: Context,
        symbols: &[&str],
        should_stop: impl Fn() -> bool,
        mut inject_rx: Option<UnboundedReceiver<Message>>,
    ) where
        P: Endpoints<M> + Sync + Clone + 'static,
        Context: Sync + Clone + 'static,
        H: EventHandler<M, Context = Context> + 'static,
    {
        let mut stream = Self::subscribe(&provider, symbols).await;
        let mut handler = Self::build(provider, symbols, context).await.unwrap();

        loop {
            if should_stop() {
                break;
            }

            tokio::select! {
                biased;

                msg = async {
                    match &mut inject_rx {
                        Some(rx) => rx.recv().await,
                        None => std::future::pending().await,
                    }
                } => {
                    match msg {
                        Some(msg) => {
                            handler.route_message(msg);
                        }
                        None => inject_rx = None,
                    }
                }

                res = stream.next(), if !should_stop() => {
                    match res {
                        Some(Ok(msg)) => {
                            handler.route_message(msg);
                        }
                        Some(Err(e)) => error!("{e}"),
                        None => break,
                    }
                }
            }
        }

        if let Err(e) = disconnect(&mut stream).await {
            debug!("Failed to disconnect {e}: {stream:?}");
        }
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::rc::Rc;

    struct TestUpdate {
        symbol: String,
    }

    struct RecordingHandler {
        seen: Rc<RefCell<Vec<String>>>,
    }

    #[allow(dead_code)]
    struct NoopEndpoints;

    impl Endpoints<()> for NoopEndpoints {
        fn websocket_url(&self) -> String {
            String::new()
        }

        fn ws_subscriptions(
            &self,
            _symbols: impl Iterator<Item = impl AsRef<str>>,
        ) -> Vec<String> {
            Vec::new()
        }

        fn rest_api_url(&self, _symbol: impl AsRef<str>) -> String {
            String::new()
        }
    }

    impl EventHandler<()> for RecordingHandler {
        type Error = String;
        type Context = Rc<RefCell<Vec<String>>>;
        type Update = TestUpdate;

        fn parse_update(value: Message) -> Result<Option<Self::Update>, Self::Error> {
            let Message::Text(text) = value else {
                return Err("expected text frame".into());
            };
            let symbol = text
                .split(':')
                .nth(1)
                .ok_or_else(|| "missing symbol".to_string())?
                .to_string();
            Ok(Some(TestUpdate { symbol }))
        }

        fn to_id(event: &Self::Update) -> &str {
            &event.symbol
        }

        fn handle_update(&mut self, event: Self::Update) -> Result<(), Self::Error> {
            self.seen.borrow_mut().push(event.symbol);
            Ok(())
        }

        async fn build<En>(
            _provider: En,
            symbols: &[impl AsRef<str>],
            seen: Self::Context,
        ) -> Result<(String, Self), Self::Error>
        where
            En: Endpoints<()> + Clone + 'static,
        {
            let symbol = symbols
                .first()
                .expect("one symbol")
                .as_ref()
                .to_string();
            Ok((
                symbol.clone(),
                RecordingHandler { seen: seen.clone() },
            ))
        }
    }

    #[test]
    fn route_message_invokes_matching_symbol_handler() {
        let seen = Rc::new(RefCell::new(Vec::new()));
        let mut mux = MonitorMultiplexor::<RecordingHandler, ()> {
            writers: HashMap::from([
                (
                    "BTCUSDT".into(),
                    RecordingHandler {
                        seen: seen.clone(),
                    },
                ),
                (
                    "ETHUSDT".into(),
                    RecordingHandler {
                        seen: seen.clone(),
                    },
                ),
            ]),
            _p: PhantomData,
        };

        let outcome = mux.route_message(Message::Text("depth:BTCUSDT:1".into()));
        assert_eq!(outcome, RouteOutcome::Handled);
        assert_eq!(*seen.borrow(), vec!["BTCUSDT".to_string()]);

        let outcome = mux.route_message(Message::Text("depth:ETHUSDT:2".into()));
        assert_eq!(outcome, RouteOutcome::Handled);
        assert_eq!(
            *seen.borrow(),
            vec!["BTCUSDT".to_string(), "ETHUSDT".to_string()]
        );
    }

    #[test]
    fn route_message_reports_missing_handler() {
        let seen = Rc::new(RefCell::new(Vec::new()));
        let mut mux = MonitorMultiplexor::<RecordingHandler, ()> {
            writers: HashMap::from([(
                "BTCUSDT".into(),
                RecordingHandler {
                    seen: seen.clone(),
                },
            )]),
            _p: PhantomData,
        };

        let outcome = mux.route_message(Message::Text("depth:SOLUSDT:1".into()));
        assert_eq!(outcome, RouteOutcome::MissingHandler);
        assert!(seen.borrow().is_empty());
    }

    #[tokio::test]
    async fn ingress_push_delivers_to_handler() {
        let seen = Rc::new(RefCell::new(Vec::new()));
        let mut mux = MonitorMultiplexor::<RecordingHandler, ()> {
            writers: HashMap::from([(
                "BTCUSDT".into(),
                RecordingHandler {
                    seen: seen.clone(),
                },
            )]),
            _p: PhantomData,
        };

        let (ingress, mut rx) = message_ingress();
        ingress
            .push(Message::Text("depth:BTCUSDT:42".into()))
            .unwrap();

        let msg = rx.recv().await.expect("ingress message");
        let outcome = mux.route_message(msg);
        assert_eq!(outcome, RouteOutcome::Handled);
        assert_eq!(*seen.borrow(), vec!["BTCUSDT".to_string()]);
    }
}
