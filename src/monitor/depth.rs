use clap::{Args, ValueEnum};
pub use lob::DepthUpdate;

pub struct Depth;

/// How depth updates are consumed after parsing.
#[derive(Clone, Copy, Debug, Default, ValueEnum, Eq, PartialEq)]
pub enum DepthOutput {
    /// Maintain books and serve them (gRPC and/or SCALE); requires `grpc` / `codec` features.
    #[default]
    Serve,
    /// Print each update to stdout; no REST snapshot and no server.
    Echo,
}

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
    #[arg(long, help = "Serve the book at this port (only for --output serve).")]
    server_port: Option<u16>,
    #[arg(long, value_enum, default_value_t = DepthOutput::Serve)]
    output: DepthOutput,
}

/// Run the echo depth pipeline (WebSocket + print); same logic as `monitor depth --output echo`.
pub async fn stream_depth_echo(provider: super::Provider, symbols: &str) {
    use crate::connectors::multiplexor::MonitorMultiplexor;

    let symbols = symbols.to_uppercase();
    let syms: Vec<&str> = symbols
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect();

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async move {
            match provider {
                super::Provider::Binance => {
                    MonitorMultiplexor::<super::echo_depth::EchoDepth, Depth>::stream::<_, _>(
                        crate::providers::Binance,
                        (),
                        &syms,
                    )
                    .await
                }
                super::Provider::BinanceUsdM => {
                    MonitorMultiplexor::<super::echo_depth::EchoDepth, Depth>::stream::<_, _>(
                        crate::providers::BinanceUsdM,
                        (),
                        &syms,
                    )
                    .await
                }
                super::Provider::Other => todo!("unknown provider"),
            }
        })
        .await;
}

#[cfg(any(feature = "codec", feature = "grpc"))]
async fn stream_depth_serve(cfg: &DepthConfig) {
    use crate::{
        connectors::multiplexor::MonitorMultiplexor,
        monitor::{order_book::OrderBook, Provider},
    };

    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    let port = cfg.server_port.unwrap_or(50051u16);
    let n = cfg.symbols.split(',').count();
    std::thread::spawn(move || crate::servers::start(rx, port, n));

    let symbols = cfg.symbols.to_uppercase();
    let syms: Vec<&str> = symbols.split(',').collect();

    match cfg.provider {
        Provider::Binance => {
            MonitorMultiplexor::<OrderBook, Depth>::stream::<_, _>(
                crate::providers::Binance,
                tx,
                &syms,
            )
            .await
        }
        Provider::BinanceUsdM => {
            MonitorMultiplexor::<OrderBook, Depth>::stream::<_, _>(
                crate::providers::BinanceUsdM,
                tx,
                &syms,
            )
            .await
        }
        super::Provider::Other => todo!(),
    }
}

#[cfg(not(any(feature = "codec", feature = "grpc")))]
async fn stream_depth_serve(_cfg: &DepthConfig) {
    tracing::error!(
        "depth --output serve requires the `grpc` and/or `codec` feature; rebuild with default features or `--features grpc,codec`"
    );
}

impl super::Monitor for DepthConfig {
    async fn monitor(&self) {
        match self.output {
            DepthOutput::Echo => stream_depth_echo(self.provider.clone(), &self.symbols).await,
            DepthOutput::Serve => {
                let local = tokio::task::LocalSet::new();
                local.run_until(stream_depth_serve(self)).await;
            }
        }
    }
}
