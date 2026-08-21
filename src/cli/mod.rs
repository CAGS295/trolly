use clap::{Parser, Subcommand, ValueEnum};

use binance_spot_exec::{
    ApiCredentials, NativeTlsTransport, OrderSide, PlaceOrderRequest, SpotOrderClient, TimeInForce,
};

use crate::policy_demo::{
    run_policy_demo, DemoVenue, PolicyDemoConfig, PolicyDemoOrders, PolicyDemoReport,
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
    /// Run a guarded policy-to-demo-execution bridge.
    PolicyDemo(PolicyDemoArgs),
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

#[derive(Parser, Debug)]
struct PolicyDemoArgs {
    /// Execution venue adapter to exercise.
    #[clap(long, value_enum, default_value_t = PolicyDemoVenue::Spot)]
    venue: PolicyDemoVenue,
    /// Trading pair (e.g. BTCUSDT).
    #[clap(long, default_value = "BTCUSDT")]
    symbol: String,
    /// Default order quantity emitted by Buy/Sell actions.
    #[clap(long, default_value = "0.01")]
    qty: String,
    /// Observation window frame count.
    #[clap(long, default_value_t = 1)]
    window_frames: usize,
    /// Number of synthetic normalized depth observations to feed.
    #[clap(long, default_value_t = 3)]
    max_steps: usize,
    /// Place generated requests on Binance demo REST. Requires RUN_BINANCE_DEMO_ORDERS=1.
    #[clap(long)]
    execute_demo_orders: bool,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum PolicyDemoVenue {
    Spot,
    Usdm,
}

impl From<PolicyDemoVenue> for DemoVenue {
    fn from(value: PolicyDemoVenue) -> Self {
        match value {
            PolicyDemoVenue::Spot => DemoVenue::Spot,
            PolicyDemoVenue::Usdm => DemoVenue::Usdm,
        }
    }
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
            Self::PolicyDemo(args) => args.run().await,
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

impl PolicyDemoArgs {
    async fn run(&self) {
        let mut config = PolicyDemoConfig::new(self.venue.into(), self.symbol.clone());
        config.qty = self.qty.clone();
        config.window_frames = self.window_frames;
        config.max_steps = self.max_steps;
        config.execute_demo_orders = self.execute_demo_orders;

        match run_policy_demo(config).await {
            Ok(report) => print_policy_demo_report(&report),
            Err(err) => {
                eprintln!("policy demo failed: {err}");
                std::process::exit(1);
            }
        }
    }
}

fn print_policy_demo_report(report: &PolicyDemoReport) {
    println!(
        "policy demo: venue={} symbol={} policy={} steps={} order_requests={} placed_orders={} mode={}",
        report.venue,
        report.symbol,
        report.policy_source,
        report.steps,
        report.order_count(),
        report.placed_orders,
        if report.execute_demo_orders { "demo-orders" } else { "dry-run" },
    );

    match &report.orders {
        PolicyDemoOrders::Spot(orders) => {
            for (idx, order) in orders.iter().enumerate() {
                println!("spot_order[{idx}]: {order:?}");
            }
        }
        PolicyDemoOrders::Usdm(orders) => {
            for (idx, order) in orders.iter().enumerate() {
                println!("usdm_order[{idx}]: {order:?}");
            }
        }
    }
}
