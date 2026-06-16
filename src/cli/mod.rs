use clap::{Parser, Subcommand};
use tracing::{error, info};

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
    /// Place orders on Binance spot (REST outbound; fills via user-data stream).
    Execute {
        #[clap(subcommand)]
        action: ExecuteCommands,
    },
}

#[derive(Subcommand, Debug)]
enum ExecuteCommands {
    /// Submit a signed market or limit order.
    PlaceOrder(PlaceOrderArgs),
}

#[derive(Parser, Debug)]
struct PlaceOrderArgs {
    /// Trading pair, e.g. BTCUSDT
    #[clap(long)]
    symbol: String,
    /// BUY or SELL
    #[clap(long)]
    side: String,
    /// Order quantity
    #[clap(long)]
    quantity: String,
    /// Limit price (omit for market order)
    #[clap(long)]
    price: Option<String>,
    /// Time in force for limit orders: GTC, IOC, FOK (default GTC)
    #[clap(long, default_value = "GTC")]
    time_in_force: String,
    /// REST base URL (default production; use testnet URL for demo keys)
    #[clap(long, default_value = binance_spot_exec::BinanceSpotRest::PRODUCTION_URL)]
    rest_url: String,
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
            Self::Execute {
                action: ExecuteCommands::PlaceOrder(args),
            } => run_place_order(args).await,
        };
    }
}

async fn run_place_order(args: &PlaceOrderArgs) {
    let api_key = std::env::var("DEMO_BINANCE_KEY")
        .or_else(|_| std::env::var("BINANCE_API_KEY"))
        .unwrap_or_else(|_| {
            error!("set DEMO_BINANCE_KEY or BINANCE_API_KEY");
            std::process::exit(1);
        });
    let secret_key = std::env::var("DEMO_BINANCE_SECRET")
        .or_else(|_| std::env::var("BINANCE_SECRET_KEY"))
        .unwrap_or_else(|_| {
            error!("set DEMO_BINANCE_SECRET or BINANCE_SECRET_KEY");
            std::process::exit(1);
        });

    let order = match binance_spot_exec::build_order(
        &args.symbol,
        &args.side,
        &args.quantity,
        args.price.as_deref(),
        args.price
            .as_ref()
            .map(|_| args.time_in_force.as_str()),
    ) {
        Ok(order) => order,
        Err(e) => {
            error!("invalid order: {e}");
            std::process::exit(1);
        }
    };

    let client = binance_spot_exec::SpotOrderClient::with_base_url(
        &args.rest_url,
        binance_spot_exec::ApiCredentials {
            api_key,
            secret_key,
        },
    );

    match client.place_order(&order).await {
        Ok(response) => {
            info!(
                order_id = response.order_id,
                client_order_id = %response.client_order_id,
                status = %response.status,
                symbol = %response.symbol,
                "order accepted (reconcile fills via user-data executionReport)"
            );
        }
        Err(e) => {
            error!("order rejected: {e}");
            std::process::exit(1);
        }
    }
}
