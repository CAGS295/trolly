use crate::net::streaming_strategy::{Error, EventHandler, Message};
pub use crate::net::ws_adapter::{connect, disconnect};
use crate::providers::Provider;
use async_trait::async_trait;
use clap::{Args, Subcommand};
use log::{error, info};

#[derive(Subcommand, Debug)]
pub enum Monitorables {
    /// Depth of the symbol.
    Depth(DepthArgs),
}

#[derive(Args, Debug)]
pub struct DepthArgs {
    #[arg(value_enum)]
    #[arg(short, long)]
    provider: Provider,
    #[arg(
        short,
        long,
        help = "web socket RPC to pull data from. e.g. wss://127.0.0.1:9944"
    )]
    pub(crate) ws_url: String,
}

#[async_trait]
impl EventHandler for DepthArgs {
    async fn handle_event(&self, event: Result<Message, Error>) -> Result<(), ()> {
        match event {
            Ok(message) => {
                info!("Message : {message}");
                Ok(())
            }
            Err(e) => {
                error!("{e}");
                Err(())
            }
        }
    }
}
