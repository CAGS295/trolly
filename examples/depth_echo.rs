//! Minimal depth client: subscribe and print updates (no gRPC / SCALE).
//!
//! ```text
//! cargo run --example depth_echo -- --provider binance-usd-m --symbols rpi:BTCUSDC,BTCUSDC
//! ```

use clap::Parser;
use tracing_subscriber::{fmt, prelude::__tracing_subscriber_SubscriberExt, util::SubscriberInitExt, EnvFilter};

#[derive(Parser, Debug)]
#[command(about = "Echo Binance-style depth updates to stdout")]
struct Args {
    #[arg(short, long, value_enum)]
    provider: trolly::monitor::Provider,
    #[arg(short, long)]
    symbols: String,
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> color_eyre::Result<()> {
    color_eyre::install()?;
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info,trolly=info"));
    tracing_subscriber::registry()
        .with(fmt::layer())
        .with(filter)
        .init();

    let args = Args::parse();
    trolly::monitor::stream_depth_echo(args.provider, &args.symbols).await;
    Ok(())
}
