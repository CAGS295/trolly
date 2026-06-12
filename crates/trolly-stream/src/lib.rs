//! Shared stream ingress, routing, and websocket adapters.

mod endpoints;
mod handler;
mod multiplexor;
mod ws_adapter;

pub use endpoints::{StreamEndpoints, VenueEndpoints};
pub use handler::{EventHandler, Message};
pub use multiplexor::MonitorMultiplexor;
pub use ws_adapter::{connect, disconnect};
