use super::{order_book::OrderBook, Provider};
use crate::net::streaming::EventHandler;
use crate::net::streaming::SimpleStream;
use crate::providers::{Binance, Depth, Endpoints};
use async_trait::async_trait;
use clap::Args;
use lob::LimitOrderBook;
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
}

impl DepthConfig {
    fn select_provider(&self) -> impl Endpoints<Depth> {
        match self.provider {
            Provider::Binance => Binance,
            _ => unimplemented!(),
        }
    }

    pub(crate) async fn to_event_handler(
        &self,
        provider: &impl Endpoints<Depth>,
    ) -> reqwest::Result<impl EventHandler> {
        let url = provider.rest_api_url(&self.symbol);
        let response: reqwest::Response = reqwest::get(url).await?;
        if let Err(e) = response.error_for_status_ref() {
            error!("Query failed:{e} {}", response.text().await?);
            return Err(e);
        };
        let lob: LimitOrderBook = response.json().await?;
        Ok(OrderBook::from(lob))
    }
}

#[async_trait]
impl super::Monitor for DepthConfig {
    async fn monitor(&self) {
        let provider = self.select_provider();
        let mut handler = self.to_event_handler(&provider).await.unwrap();
        let stream = SimpleStream {
            url: &provider.websocket_url(&self.symbol),
        };
        stream.stream(&mut handler).await;
    }
}
