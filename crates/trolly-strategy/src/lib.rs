//! Strategy runtime: consume multi-symbol stream events, hold state, dispatch outbound messages.

mod egress;
mod event;
mod runtime;
mod strategy;

pub use egress::{OutboundMessage, RecordingEgress, StreamEgress};
pub use event::{
    AccountUpdate, DepthUpdate, EventKind, ExecutionUpdate, PriceLevel, StreamEvent,
};
pub use runtime::{
    envelope_message, parse_envelope, route_message, IngestError, ParseError,
    StrategyEventHandler, StrategyHub, StrategyRuntime,
};
pub use strategy::{RecordingStrategy, Strategy};
