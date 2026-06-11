//! Shared stream multiplexor, websocket ingress, and event routing.

mod endpoints;
mod handler;
mod multiplexor;
mod ws_adapter;

pub use endpoints::Endpoints;
pub use handler::{connect, disconnect, EventHandler, Message};
pub use multiplexor::{
    message_ingress, MessageIngress, MonitorMultiplexor, RouteOutcome,
};

/// Crate identifier for workspace wiring tests.
pub const CRATE: &str = env!("CARGO_PKG_NAME");

#[cfg(test)]
mod workspace_wiring {
    #[test]
    fn crate_name() {
        assert_eq!(super::CRATE, "trolly-stream");
    }
}
