use clap::{Parser, Subcommand, ValueEnum};
use binance_spot_exec::{
    ApiCredentials, OrderSide, SpotOrderClient, SpotOrderRequest, DEFAULT_REST_BASE_URL,
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
    /// Place a signed Binance spot order via REST (fills reconcile on user-data stream).
    Execute(ExecuteArgs),
}

#[derive(Parser, Debug)]
pub struct ExecuteArgs {
    /// Trading pair, e.g. BTCUSDT.
    #[clap(long)]
    pub symbol: String,
    /// Order side.
    #[clap(long, value_enum)]
    pub side: ExecuteSide,
    /// Order quantity.
    #[clap(long)]
    pub qty: String,
    /// Limit price; omit for market orders.
    #[clap(long)]
    pub price: Option<String>,
    /// Binance API key (`DEMO_BINANCE_KEY` from `.env`).
    #[clap(long, env = "DEMO_BINANCE_KEY")]
    pub api_key: String,
    /// Binance API secret (`DEMO_BINANCE_SECRET` from `.env`).
    #[clap(long, env = "DEMO_BINANCE_SECRET")]
    pub api_secret: String,
    /// REST base URL (production default; use https://demo-api.binance.com for demo).
    #[clap(long, default_value = DEFAULT_REST_BASE_URL)]
    pub base_url: String,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
pub enum ExecuteSide {
    Buy,
    Sell,
}

impl ExecuteSide {
    fn to_order_side(self) -> OrderSide {
        match self {
            Self::Buy => OrderSide::Buy,
            Self::Sell => OrderSide::Sell,
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
            Self::Execute(args) => args.run().await,
        };
    }
}

impl ExecuteArgs {
    async fn run(&self) {
        let credentials = ApiCredentials {
            api_key: self.api_key.clone(),
            secret_key: self.api_secret.clone(),
        };
        let client = SpotOrderClient::new(&self.base_url, credentials);
        let request = match &self.price {
            Some(price) => SpotOrderRequest::limit(
                &self.symbol,
                self.side.to_order_side(),
                &self.qty,
                price,
            ),
            None => SpotOrderRequest::market(&self.symbol, self.side.to_order_side(), &self.qty),
        };

        match client.place_order(request).await {
            Ok(response) => {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&response).unwrap_or_else(|err| err.to_string())
                );
            }
            Err(err) => {
                eprintln!("order placement failed: {err}");
                std::process::exit(1);
            }
        }
    }
}
