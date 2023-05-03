use super::{
    order_book::{Operations, OrderBook},
    Provider,
};
use crate::net::streaming::EventHandler;
use crate::net::streaming::SimpleStream;
use crate::providers::{Binance, Depth, Endpoints};
use async_trait::async_trait;
use clap::Args;
use left_right::WriteHandle;
use lob::LimitOrderBook;
use std::{error::Error, thread};
use tracing::error;

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
    symbol: String,
    #[arg(long, help = "Serve the book at this port.")]
    server_port: Option<u16>,
}

impl DepthConfig {
    fn select_provider(&self) -> impl Endpoints<Depth> {
        match self.provider {
            Provider::Binance => Binance,
            _ => unimplemented!(),
        }
    }

    pub(crate) async fn event_handler_builder(
        &self,
        provider: &impl Endpoints<Depth>,
        mut writer: WriteHandle<LimitOrderBook, Operations>,
    ) -> Result<impl EventHandler, impl Error> {
        let url = provider.rest_api_url(&self.symbol);
        let response: reqwest::Response = reqwest::get(url).await?;
        if let Err(e) = response.error_for_status_ref() {
            error!("Query failed:{e} {}", response.text().await?);
            return Err(e);
        };
        let lob: LimitOrderBook = response.json().await?;
        writer.append(Operations::Initialize(lob));
        Ok(OrderBook::from(writer))
    }
}

#[async_trait]
impl super::Monitor for DepthConfig {
    async fn monitor(&self) {
        let provider = self.select_provider();
        let (w, r) = left_right::new();
        let handler_builder = || self.event_handler_builder(&provider, w);
        let port = self.server_port.unwrap_or(50051u16);
        thread::spawn(move || crate::tonic::start(r.factory(), port));
        let stream = SimpleStream {
            url: &provider.websocket_url(&self.symbol),
        };
        stream.stream(handler_builder).await;
    }
}
