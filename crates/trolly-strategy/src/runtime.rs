//! Strategy runtime: ingest events, apply transitions, dispatch egress.

use std::cell::RefCell;
use std::future::Future;
use std::rc::Rc;

use trolly_stream::{EventHandler, Message, VenueEndpoints};

use crate::egress::StreamEgress;
use crate::event::StreamEvent;
use crate::strategy::Strategy;

/// Parse error for normalized stream envelopes.
#[derive(Debug, Clone, PartialEq)]
pub enum ParseError {
    NotText,
    InvalidJson(String),
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotText => write!(f, "expected text websocket frame"),
            Self::InvalidJson(msg) => write!(f, "invalid envelope json: {msg}"),
        }
    }
}

impl std::error::Error for ParseError {}

/// Single core state-handling unit wired to ingress and egress.
pub struct StrategyRuntime<S, E>
where
    S: Strategy,
    E: StreamEgress,
{
    strategy: S,
    egress: E,
}

impl<S, E> StrategyRuntime<S, E>
where
    S: Strategy,
    E: StreamEgress,
{
    pub fn new(strategy: S, egress: E) -> Self {
        Self { strategy, egress }
    }

    /// Consume one normalized event, apply strategy transitions, dispatch outbound messages.
    pub fn ingest(&mut self, event: StreamEvent) -> Result<(), E::Error> {
        let commands = self.strategy.on_event(&event);
        for command in commands {
            self.egress.dispatch(command)?;
        }
        Ok(())
    }

    pub fn strategy(&self) -> &S {
        &self.strategy
    }

    pub fn strategy_mut(&mut self) -> &mut S {
        &mut self.strategy
    }

    pub fn egress(&self) -> &E {
        &self.egress
    }

    pub fn egress_mut(&mut self) -> &mut E {
        &mut self.egress
    }
}

/// Hub that accepts injected websocket frames or pre-parsed events.
pub struct StrategyHub<S, E>
where
    S: Strategy,
    E: StreamEgress,
{
    runtime: StrategyRuntime<S, E>,
}

impl<S, E> StrategyHub<S, E>
where
    S: Strategy,
    E: StreamEgress,
{
    pub fn new(strategy: S, egress: E) -> Self {
        Self {
            runtime: StrategyRuntime::new(strategy, egress),
        }
    }

    pub fn ingest_event(&mut self, event: StreamEvent) -> Result<(), E::Error> {
        self.runtime.ingest(event)
    }

    /// Parse a normalized JSON envelope and feed the runtime.
    pub fn ingest_message(&mut self, msg: Message) -> Result<(), IngestError<E::Error>> {
        let event = parse_envelope(msg).map_err(IngestError::Parse)?;
        self.runtime.ingest(event).map_err(IngestError::Egress)
    }

    pub fn runtime(&self) -> &StrategyRuntime<S, E> {
        &self.runtime
    }

    pub fn runtime_mut(&mut self) -> &mut StrategyRuntime<S, E> {
        &mut self.runtime
    }
}

#[derive(Debug)]
pub enum IngestError<E> {
    Parse(ParseError),
    Egress(E),
}

impl<E: std::fmt::Debug> std::fmt::Display for IngestError<E> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Parse(e) => write!(f, "{e}"),
            Self::Egress(e) => write!(f, "egress error: {e:?}"),
        }
    }
}

impl<E: std::fmt::Debug> std::error::Error for IngestError<E> {}

/// Parse a normalized [`StreamEvent`] from a websocket text frame.
pub fn parse_envelope(msg: Message) -> Result<StreamEvent, ParseError> {
    let Message::Text(text) = msg else {
        return Err(ParseError::NotText);
    };
    serde_json::from_str(&text).map_err(|e| ParseError::InvalidJson(e.to_string()))
}

/// Serialize a normalized event into an injectable websocket text frame.
pub fn envelope_message(event: &StreamEvent) -> Message {
    Message::Text(
        serde_json::to_string(event)
            .expect("stream event serializes")
            .into(),
    )
}

type SharedRuntime<S, E> = Rc<RefCell<StrategyRuntime<S, E>>>;

/// [`EventHandler`] adapter that fans multi-symbol ingress into one shared runtime.
pub struct StrategyEventHandler<S, E>
where
    S: Strategy + 'static,
    E: StreamEgress + 'static,
{
    runtime: SharedRuntime<S, E>,
}

impl<S, E> StrategyEventHandler<S, E>
where
    S: Strategy + 'static,
    E: StreamEgress + 'static,
{
    pub fn new(strategy: S, egress: E) -> Self {
        Self {
            runtime: Rc::new(RefCell::new(StrategyRuntime::new(strategy, egress))),
        }
    }

    pub fn shared_runtime(&self) -> SharedRuntime<S, E> {
        Rc::clone(&self.runtime)
    }
}

impl<S, E> Clone for StrategyEventHandler<S, E>
where
    S: Strategy + 'static,
    E: StreamEgress + 'static,
{
    fn clone(&self) -> Self {
        Self {
            runtime: Rc::clone(&self.runtime),
        }
    }
}

impl<S, E> EventHandler<()> for StrategyEventHandler<S, E>
where
    S: Strategy + 'static,
    E: StreamEgress + 'static,
    E::Error: std::fmt::Debug,
{
    type Error = IngestError<E::Error>;
    type Context = SharedRuntime<S, E>;
    type Update = StreamEvent;

    fn parse_update(value: Message) -> Result<Option<Self::Update>, Self::Error> {
        parse_envelope(value)
            .map(Some)
            .map_err(IngestError::Parse)
    }

    fn to_id(event: &Self::Update) -> &str {
        event.routing_id()
    }

    fn handle_update(&mut self, event: Self::Update) -> Result<(), Self::Error> {
        self.runtime
            .borrow_mut()
            .ingest(event)
            .map_err(IngestError::Egress)
    }

    fn build<En>(
        _provider: En,
        symbols: &[impl AsRef<str>],
        runtime: Self::Context,
    ) -> impl Future<Output = Result<(String, Self), Self::Error>>
    where
        En: VenueEndpoints + Clone + 'static,
        Self: Sized,
    {
        let symbol = symbols.first().unwrap().as_ref().to_string();
        async move {
            Ok((
                symbol,
                Self {
                    runtime,
                },
            ))
        }
    }
}

/// Route one injectable websocket frame through per-symbol handlers (multiplexor contract).
pub fn route_message<H>(writers: &mut std::collections::HashMap<String, H>, msg: Message)
where
    H: EventHandler<()>,
    H::Update: std::fmt::Debug,
    H::Error: std::fmt::Debug,
{
    match msg {
        msg if msg.is_text() => match H::parse_update(msg) {
            Ok(Some(update)) => {
                let id = H::to_id(&update);
                if let Some(handler) = writers.get_mut(id) {
                    handler.handle_update(update).ok();
                }
            }
            Ok(None) => {}
            Err(_) => {}
        },
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::egress::{OutboundMessage, RecordingEgress};
    use crate::event::{AccountUpdate, DepthUpdate, EventKind, ExecutionUpdate, PriceLevel};
    use crate::strategy::RecordingStrategy;
    use std::collections::HashMap;

    fn depth_event(symbol: &str) -> StreamEvent {
        StreamEvent::Depth(DepthUpdate {
            symbol: symbol.into(),
            bids: vec![PriceLevel {
                price: "100".into(),
                qty: "1".into(),
            }],
            asks: vec![],
            update_id: Some(1),
        })
    }

    fn execution_event(symbol: &str) -> StreamEvent {
        StreamEvent::Execution(ExecutionUpdate {
            symbol: symbol.into(),
            order_id: "42".into(),
            side: "BUY".into(),
            fill_qty: "0.5".into(),
            fill_price: "100".into(),
        })
    }

    fn account_event() -> StreamEvent {
        StreamEvent::Account(AccountUpdate {
            asset: "USDT".into(),
            balance: "1000".into(),
            symbol: None,
        })
    }

    #[test]
    fn parse_envelope_roundtrip() {
        let event = depth_event("BTCUSDT");
        let parsed = parse_envelope(envelope_message(&event)).unwrap();
        assert_eq!(parsed, event);
    }

    #[test]
    fn runtime_dispatches_strategy_commands() {
        let response = OutboundMessage::Subscribe {
            symbol: "BTCUSDT".into(),
            channel: "depth".into(),
        };
        let mut hub = StrategyHub::new(
            RecordingStrategy::with_responses(vec![response.clone()]),
            RecordingEgress::default(),
        );

        hub.ingest_event(depth_event("BTCUSDT")).unwrap();

        assert_eq!(hub.runtime().strategy().consumed.len(), 1);
        assert_eq!(hub.runtime().egress().dispatched, vec![response]);
    }

    #[test]
    fn hub_ingests_multi_symbol_synthetic_messages() {
        let mut hub = StrategyHub::new(RecordingStrategy::default(), RecordingEgress::default());

        hub.ingest_message(envelope_message(&depth_event("BTCUSDT")))
            .unwrap();
        hub.ingest_message(envelope_message(&execution_event("ETHUSDT")))
            .unwrap();
        hub.ingest_message(envelope_message(&account_event())).unwrap();

        let consumed = &hub.runtime().strategy().consumed;
        assert_eq!(consumed.len(), 3);
        assert_eq!(consumed[0].kind(), EventKind::Depth);
        assert_eq!(consumed[1].kind(), EventKind::Execution);
        assert_eq!(consumed[2].kind(), EventKind::Account);
    }

    #[test]
    fn event_handler_shares_state_across_symbols() {
        let handler = StrategyEventHandler::new(RecordingStrategy::default(), RecordingEgress::default());
        let shared = handler.shared_runtime();
        let mut writers: HashMap<String, StrategyEventHandler<RecordingStrategy, RecordingEgress>> =
            HashMap::new();
        for sym in ["BTCUSDT", "ETHUSDT"] {
            writers.insert(sym.into(), handler.clone());
        }

        route_message(
            &mut writers,
            envelope_message(&depth_event("BTCUSDT")),
        );
        route_message(
            &mut writers,
            envelope_message(&execution_event("ETHUSDT")),
        );

        let guard = shared.borrow();
        let consumed = &guard.strategy().consumed;
        assert_eq!(consumed.len(), 2);
        assert_eq!(consumed[0].routing_id(), "BTCUSDT");
        assert_eq!(consumed[1].routing_id(), "ETHUSDT");
    }
}
