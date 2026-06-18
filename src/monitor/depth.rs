use clap::{Args, ValueEnum};
pub use lob::DepthUpdate;

use super::parse_book_sources;

pub struct Depth;

/// How depth updates are consumed after parsing.
#[derive(Clone, Copy, Debug, Default, ValueEnum, Eq, PartialEq)]
pub enum DepthOutput {
    /// Maintain books and serve them (gRPC and/or SCALE); requires `grpc` / `codec` features.
    #[default]
    Serve,
    /// Cross-source merged global book over gRPC / SCALE; requires `--sources`.
    Global,
    /// Print each update to stdout; no REST snapshot and no server.
    Echo,
}

#[derive(Args, Debug)]
pub struct DepthConfig {
    #[arg(value_enum)]
    #[arg(short, long)]
    provider: super::SelectableProvider,
    #[arg(
        short,
        long,
        help = "web socket RPC to pull data from. e.g. wss://strean.provider.com:9443"
    )]
    ws_url: Option<String>,
    #[arg(
        short,
        long,
        help = "comma separated pairs, e.g. btcusdt,btcusdc (not used with --output global)"
    )]
    symbols: String,
    #[arg(
        long,
        help = "Cross-source book legs: provider:SYMBOL,... (required for --output global)"
    )]
    sources: Option<String>,
    #[arg(long, help = "Serve the book at this port (only for --output serve or global).")]
    server_port: Option<u16>,
    #[arg(long, value_enum, default_value_t = DepthOutput::Serve)]
    output: DepthOutput,
}

impl DepthConfig {
    fn validate(&self) -> Result<(), String> {
        match self.output {
            DepthOutput::Global => {
                let Some(sources) = self.sources.as_deref() else {
                    return Err("--sources is required when --output global".into());
                };
                if sources.trim().is_empty() {
                    return Err("--sources must list at least one provider:SYMBOL".into());
                }
                parse_book_sources(sources)?;
            }
            DepthOutput::Serve | DepthOutput::Echo => {
                if self.symbols.split(',').all(|s| s.trim().is_empty()) {
                    return Err("--symbols must list at least one pair".into());
                }
            }
        }
        Ok(())
    }
}

/// Run the echo depth pipeline (WebSocket + print); same logic as `monitor depth --output echo`.
pub async fn stream_depth_echo(provider: super::Provider, symbols: &str) {
    use crate::connectors::stream;

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
                    stream::<super::echo_depth::EchoDepth, Depth, _, _>(
                        crate::providers::Binance,
                        (),
                        &syms,
                    )
                    .await
                }
                super::Provider::BinanceUsdM => {
                    stream::<super::echo_depth::EchoDepth, Depth, _, _>(
                        crate::providers::BinanceUsdM,
                        (),
                        &syms,
                    )
                    .await
                }
                super::Provider::Stub => {
                    stream::<super::echo_depth::EchoDepth, Depth, _, _>(
                        crate::providers::Stub,
                        (),
                        &syms,
                    )
                    .await
                }
                super::Provider::Other => tracing::error!("echo: unknown provider"),
            }
        })
        .await;
}

#[cfg(any(feature = "codec", feature = "grpc"))]
async fn stream_depth_serve(cfg: &DepthConfig) {
    use crate::{
        connectors::stream,
        monitor::{order_book::OrderBook, Provider},
    };

    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    let port = cfg.server_port.unwrap_or(50051u16);
    let n = cfg.symbols.split(',').count();
    std::thread::spawn(move || crate::servers::start(rx, port, n));

    let symbols = cfg.symbols.to_uppercase();
    let syms: Vec<&str> = symbols.split(',').collect();

    match &cfg.provider {
        Provider::Binance => {
            stream::<OrderBook, Depth, _, _>(crate::providers::Binance, tx, &syms).await
        }
        Provider::BinanceUsdM => {
            stream::<OrderBook, Depth, _, _>(crate::providers::BinanceUsdM, tx, &syms).await
        }
        Provider::Stub => {
            stream::<OrderBook, Depth, _, _>(crate::providers::Stub, tx, &syms).await
        }
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
        if let Err(e) = self.validate() {
            tracing::error!("{e}");
            return;
        }

        match self.output {
            DepthOutput::Echo => stream_depth_echo(self.provider.clone(), &self.symbols).await,
            DepthOutput::Serve => {
                let local = tokio::task::LocalSet::new();
                local.run_until(stream_depth_serve(self)).await;
            }
            DepthOutput::Global => {
                let sources = self.sources.as_deref().expect("validated");
                match super::stream_global_depth_serve(sources, self.server_port).await {
                    Ok(()) => {}
                    Err(e) => tracing::error!("global book: {e}"),
                }
            }
        }
    }
}
