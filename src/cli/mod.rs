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
    /// Trading pair, or comma-separated pairs (e.g. BTCUSDT,ETHUSDT).
    /// First symbol is the default dispatch / inventory pair; extras join the observation.
    #[clap(long, default_value = "BTCUSDT")]
    symbol: String,
    /// Optional tracked pair for Action::dispatch. Default keeps the first `--symbol`.
    /// Qty stays `--qty`; side stays the policy Buy/Sell/Hold.
    #[clap(long)]
    dispatch_symbol: Option<String>,
    /// Default order quantity emitted by Buy/Sell actions.
    #[clap(long, default_value = "0.01")]
    qty: String,
    /// Observation window frame count.
    #[clap(long, default_value_t = 1)]
    window_frames: usize,
    /// Number of depth observations to feed (synthetic fixture unless `--depth-json` or `--subscribe-public-depth`).
    #[clap(long, default_value_t = 3)]
    max_steps: usize,
    /// Captured depth JSON/NDJSON to ingest into Env instead of the synthetic tape.
    /// Accepts a normalized StreamEvent, a JSON array, NDJSON, or Binance book frames.
    #[clap(long)]
    depth_json: Option<std::path::PathBuf>,
    /// Place generated requests on Binance demo REST. Requires RUN_BINANCE_DEMO_ORDERS=1.
    #[clap(long)]
    execute_demo_orders: bool,
    /// Prefix for deterministic demo client order IDs.
    #[clap(long, default_value = "trolly-demo")]
    client_order_id_prefix: String,
    /// Captured spot executionReport or USDM ORDER_TRADE_UPDATE JSON frames to reconcile.
    #[clap(long)]
    reconcile_user_data_json: Option<std::path::PathBuf>,
    /// After guarded demo placement, wait on the Binance demo user-data stream for receipts.
    #[clap(
        long,
        requires = "execute_demo_orders",
        conflicts_with = "reconcile_user_data_json"
    )]
    wait_for_user_data: bool,
    /// Maximum seconds to wait for live demo user-data reconciliation.
    #[clap(long, default_value_t = 45)]
    user_data_timeout_secs: u64,
    /// Subscribe to Binance public depth (demo market stream matching `--venue`).
    /// Feeds frames through the WP-042 hook. `--depth-json` still wins.
    #[clap(long, requires = "public_depth_timeout_secs")]
    subscribe_public_depth: bool,
    /// Maximum seconds to collect public depth frames. Required with `--subscribe-public-depth`.
    #[clap(long)]
    public_depth_timeout_secs: Option<u64>,
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
        if let Some(symbol) = &self.dispatch_symbol {
            config.set_dispatch_symbol(symbol);
        }
        config.qty = self.qty.clone();
        config.window_frames = self.window_frames;
        config.max_steps = self.max_steps;
        config.execute_demo_orders = self.execute_demo_orders;
        config.client_order_id_prefix = self.client_order_id_prefix.clone();
        config.wait_for_user_data = self.wait_for_user_data;
        config.user_data_timeout = std::time::Duration::from_secs(self.user_data_timeout_secs);
        config.subscribe_public_depth = self.subscribe_public_depth;
        if let Some(secs) = self.public_depth_timeout_secs {
            config.public_depth_timeout = std::time::Duration::from_secs(secs);
        }
        if let Some(path) = &self.depth_json {
            match std::fs::read_to_string(path) {
                Ok(input) => config.captured_depth_json = Some(input),
                Err(err) => {
                    eprintln!("policy demo failed to read {}: {err}", path.display());
                    std::process::exit(1);
                }
            }
        }
        if let Some(path) = &self.reconcile_user_data_json {
            match std::fs::read_to_string(path) {
                Ok(input) => config.captured_user_data_json = Some(input),
                Err(err) => {
                    eprintln!("policy demo failed to read {}: {err}", path.display());
                    std::process::exit(1);
                }
            }
        }

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
        "policy demo: venue={} symbol={} dispatch={} policy={} depth={} steps={} order_requests={} placed_orders={} reconciled_orders={} mode={}",
        report.venue,
        report.display_symbols(),
        report.dispatch_symbol,
        report.policy_source,
        report.depth_source,
        report.steps,
        report.order_count(),
        report.placed_orders,
        report.reconciliations.len(),
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

    for (idx, receipt) in report.receipts.iter().enumerate() {
        println!(
            "receipt[{idx}]: venue={} symbol={} side={} order_id={} client_order_id={} status={}",
            receipt.venue,
            receipt.symbol,
            receipt.side,
            receipt.order_id,
            receipt.client_order_id,
            receipt.status,
        );
    }

    for (idx, reconciliation) in report.reconciliations.iter().enumerate() {
        println!(
            "reconciliation[{idx}]: venue={} symbol={} side={} order_id={} client_order_id={} status={} terminal={}",
            reconciliation.venue,
            reconciliation.symbol,
            reconciliation.side,
            reconciliation.order_id,
            reconciliation.client_order_id,
            reconciliation.status,
            reconciliation.terminal,
        );
    }

    for inventory in &report.extra_symbol_inventory {
        println!(
            "extra_inventory: symbol={} position={}",
            inventory.symbol, inventory.position
        );
    }
}
