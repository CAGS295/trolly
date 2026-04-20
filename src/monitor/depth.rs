use clap::Args;
pub use lob::DepthUpdate;

pub struct Depth;

#[derive(Args, Debug)]
pub struct DepthConfig {
    #[arg(value_enum)]
    #[arg(short, long)]
    provider: super::Provider,
    #[arg(
        short,
        long,
        help = "web socket RPC to pull data from. e.g. wss://strean.provider.com:9443"
    )]
    ws_url: Option<String>,
    #[arg(
        short,
        long,
        required = true,
        help = "comma separated pairs, e.g. btcusdt,btcusdc"
    )]
    symbols: String,
    #[arg(long, help = "Serve the book at this port.")]
    server_port: Option<u16>,
}

#[cfg(any(feature = "codec", feature = "grpc"))]
impl super::Monitor for DepthConfig {
    async fn monitor(&self) {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        let port = self.server_port.unwrap_or(50051u16);
        let n = self.symbols.split(',').count();
        std::thread::spawn(move || crate::servers::start(rx, port, n));

        let local = tokio::task::LocalSet::new();

        local
            .run_until(async move {
                use crate::{
                    connectors::multiplexor::MonitorMultiplexor,
                    monitor::{order_book::OrderBook, Provider},
                };

                let symbols = self.symbols.to_uppercase();

                match self.provider {
                    Provider::Binance => {
                        MonitorMultiplexor::<OrderBook, Depth>::stream::<_, _>(
                            crate::providers::Binance,
                            tx,
                            &symbols.split(",").collect::<Vec<_>>(),
                        )
                        .await
                    }
                    super::Provider::Other => todo!(),
                };
            })
            .await;
    }
}
