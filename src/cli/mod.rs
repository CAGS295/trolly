use clap::{Parser, Subcommand};
use tracing::info;

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
    /// Place orders and run execution workflows.
    Execute {
        #[clap(subcommand)]
        action: ExecuteCommands,
    },
}

#[derive(Subcommand, Debug)]
enum ExecuteCommands {
    /// Place a signed Binance spot order via REST (fills reconcile on user-data stream).
    PlaceOrder {
        /// Trading pair, e.g. BTCUSDT.
        symbol: String,
        /// BUY or SELL.
        side: String,
        /// Order quantity.
        qty: String,
        /// Limit price (omit for market orders).
        #[clap(long)]
        price: Option<String>,
        /// Limit order time-in-force: GTC, IOC, or FOK (default GTC).
        #[clap(long, default_value = "GTC")]
        time_in_force: String,
    },
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
            Self::Execute { action } => action.run().await,
        };
    }
}

impl Run for ExecuteCommands {
    async fn run(&self) {
        match self {
            Self::PlaceOrder {
                symbol,
                side,
                qty,
                price,
                time_in_force,
            } => place_spot_order(symbol, side, qty, price.as_deref(), time_in_force).await,
        }
    }
}

async fn place_spot_order(
    symbol: &str,
    side: &str,
    qty: &str,
    price: Option<&str>,
    time_in_force: &str,
) {
    use binance_spot_exec::{
        order_from_outbound, ApiCredentials, PlaceOrderError, SpotOrderClient,
    };

    let api_key = match std::env::var("DEMO_BINANCE_KEY") {
        Ok(value) if !value.is_empty() => value,
        _ => {
            tracing::error!(
                "DEMO_BINANCE_KEY is required; copy .env.example to .env and set demo credentials"
            );
            return;
        }
    };
    let secret_key = match std::env::var("DEMO_BINANCE_SECRET") {
        Ok(value) if !value.is_empty() => value,
        _ => {
            tracing::error!("DEMO_BINANCE_SECRET is required for signed spot order placement");
            return;
        }
    };

    let order = match order_from_outbound(
        symbol.to_string(),
        side,
        qty,
        price,
        price.map(|_| time_in_force),
    ) {
        Ok(order) => order,
        Err(err) => {
            tracing::error!("invalid order: {err}");
            return;
        }
    };

    let client = SpotOrderClient::new(ApiCredentials {
        api_key,
        secret_key,
    });

    match client.place_order(&order).await {
        Ok(result) => info!(
            order_id = result.order_id,
            status = %result.status,
            symbol = %result.symbol,
            "spot order accepted; await executionReport on user-data stream for fills"
        ),
        Err(PlaceOrderError::Api { code, msg }) => {
            tracing::error!(code, msg, "binance rejected spot order");
        }
        Err(err) => tracing::error!(error = %err, "spot order placement failed"),
    }
}
