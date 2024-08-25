use super::{order_book::OrderBook, Provider};
use crate::{
    net::streaming::MultiSymbolStream,
    providers::{Binance, Endpoints},
};
use clap::Args;
pub use lob::DepthUpdate;
use std::{fmt::Debug, future::Future, thread};
use tokio::sync::mpsc::unbounded_channel;
use tokio::task::LocalSet;

pub struct Depth;

#[derive(Args, Debug)]
pub struct DepthConfig {
    #[arg(value_enum)]
    #[arg(short, long)]
    provider: Provider,
    #[arg(
        short,
        long,
        help = "web socket RPC to pull data from. e.g. wss://strean.provider.com:9443"
    )]
    ws_url: Option<String>,
    #[arg(short, long, help = "e.g. btcusdt")]
    symbols: Vec<String>,
    #[arg(long, help = "Serve the book at this port.")]
    server_port: Option<u16>,
}

impl DepthConfig {
    fn select_provider(&self) -> impl Endpoints<Depth> + Clone {
        match self.provider {
            Provider::Binance => Binance,
            _ => unimplemented!(),
        }
    }
}

impl super::Monitor for DepthConfig {
    async fn monitor(&self) {
        let provider = self.select_provider();

        let (tx, rx) = unbounded_channel();

        let port = self.server_port.unwrap_or(50051u16);
        let n = self.symbols.len();
        thread::spawn(move || crate::servers::start(rx, port, n));

        let local = LocalSet::new();

        local
            .run_until(async move {
                MultiSymbolStream::stream::<Depth, OrderBook, _, _>(
                    provider,
                    tx,
                    self.symbols.iter().map(|s| s.to_uppercase()).collect(),
                )
                .await
            })
            .await;
    }
}

pub trait DepthHandler {
    type Error: Debug;
    type Context;

    fn handle_update(&mut self, update: DepthUpdate) -> Result<(), Self::Error>;

    fn build<En>(
        provider: En,
        symbols: &[String],
        sender: Self::Context,
    ) -> impl Future<Output = Result<Self, Self::Error>>
    where
        En: Endpoints<Depth>,
        Self: Sized;
}
