use clap::{Parser, Subcommand, ValueEnum};

use binance_spot_exec::{
    ApiCredentials, OrderSide, PlaceOrderRequest, NativeTlsTransport, SpotOrderClient, TimeInForce,
};

#[derive(Parser)]
#[clap(
    about = "Toy streamer client for crypto applications.",
    long_about = "Toy streamer client for crypto applications.\n\n\
        Goals: build a global order book; stream-native execution and account \
        bookkeeping on Binance spot and USDM (no REST); a strategy layer that \
        consumes multi-symbol stream events and dispatches outbound messages; \
        groundwork for a libtorch.rs training gym fed by trolly streams."
)]
pub struct Cli {
    #[clap(subcommand)]
    command: Commands,
    #[clap(long)]
    pub enable_telemetry: bool,
}

impl Cli {
    pub async fn start(&self) {
        self.command.run().await
    }
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Stream data from an exchange and monitor a metric or structure.
    Monitor {
        #[clap(subcommand)]
        metric: super::monitor::Monitorables,
    },
    /// Place orders on an exchange.
    Execute {
        #[clap(subcommand)]
        command: ExecuteCommands,
    },
}

#[derive(Subcommand, Debug)]
enum ExecuteCommands {
    /// Place a spot order via signed REST.
    PlaceOrder(PlaceOrderArgs),
}

#[derive(Parser, Debug)]
struct PlaceOrderArgs {
    /// Trading pair (e.g. BTCUSDT).
    #[clap(long)]
    symbol: String,
    /// Order side.
    #[clap(long, value_enum)]
    side: CliOrderSide,
    /// Order quantity.
    #[clap(long)]
    qty: String,
    /// Limit price (omit for market orders).
    #[clap(long)]
    price: Option<String>,
    /// Time in force for limit orders (default GTC).
    #[clap(long, value_enum, default_value_t = CliTimeInForce::Gtc)]
    time_in_force: CliTimeInForce,
    /// Binance API key (or set `BINANCE_API_KEY`).
    #[clap(long, env = "BINANCE_API_KEY")]
    api_key: String,
    /// Binance API secret (or set `BINANCE_SECRET_KEY`).
    #[clap(long, env = "BINANCE_SECRET_KEY")]
    secret_key: String,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum CliOrderSide {
    Buy,
    Sell,
}

impl From<CliOrderSide> for OrderSide {
    fn from(value: CliOrderSide) -> Self {
        match value {
            CliOrderSide::Buy => OrderSide::Buy,
            CliOrderSide::Sell => OrderSide::Sell,
        }
    }
}

#[derive(Clone, Copy, Debug, ValueEnum, Default)]
enum CliTimeInForce {
    #[default]
    Gtc,
    Ioc,
    Fok,
}

impl From<CliTimeInForce> for TimeInForce {
    fn from(value: CliTimeInForce) -> Self {
        match value {
            CliTimeInForce::Gtc => TimeInForce::Gtc,
            CliTimeInForce::Ioc => TimeInForce::Ioc,
            CliTimeInForce::Fok => TimeInForce::Fok,
        }
    }
}

pub trait Run {
    async fn run(&self);
}

impl Run for Commands {
    async fn run(&self) {
        match self {
            Self::Monitor {
                metric: super::monitor::Monitorables::Depth(args),
            } => {
                use super::monitor::Monitor;
                args.monitor().await;
            }
            Self::Execute { command } => command.run().await,
        };
    }
}

impl Run for ExecuteCommands {
    async fn run(&self) {
        match self {
            Self::PlaceOrder(args) => args.run().await,
        }
    }
}

impl PlaceOrderArgs {
    async fn run(&self) {
        let request = if let Some(price) = &self.price {
            PlaceOrderRequest::limit(
                &self.symbol,
                self.side.into(),
                &self.qty,
                price,
                self.time_in_force.into(),
            )
        } else {
            PlaceOrderRequest::market(&self.symbol, self.side.into(), &self.qty)
        };

        let client = SpotOrderClient::new(
            ApiCredentials {
                api_key: self.api_key.clone(),
                secret_key: self.secret_key.clone(),
            },
            NativeTlsTransport::new(),
        );

        match client.place_order(request).await {
            Ok(ack) => {
                println!(
                    "order placed: id={} symbol={} status={} side={} qty={}",
                    ack.order_id, ack.symbol, ack.status, ack.side, ack.orig_qty
                );
            }
            Err(err) => {
                eprintln!("order placement failed: {err}");
                std::process::exit(1);
            }
        }
    }
}
