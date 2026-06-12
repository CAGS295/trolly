//! Thin wrappers over [`trolly_stream`] for the root application crate.

pub use trolly_stream::{
    connect, disconnect, EventHandler, Message, MonitorMultiplexor, StreamEndpoints, VenueEndpoints,
};

use crate::signals::Terminate;

/// Run a multiplexed websocket stream until Ctrl+C, routing messages through [`MonitorMultiplexor::ingest_message`].
pub async fn stream<H, M, P, Context>(provider: P, context: Context, symbols: &[&str])
where
    P: VenueEndpoints + Sync + Clone + 'static,
    Context: Sync + Clone + 'static,
    H: EventHandler<M, Context = Context> + 'static,
{
    let shutdown = Terminate::new();
    MonitorMultiplexor::<H, M>::stream(provider, context, symbols, shutdown.flag()).await
}
