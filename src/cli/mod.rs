use clap::{Parser, Subcommand};

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
    /// Place a spot order on Binance via REST POST /api/v3/order.
    ///
    /// Credentials are read from the BINANCE_API_KEY and BINANCE_SECRET_KEY
    /// environment variables (or --api-key / --secret-key flags).
    /// Fills and rejects are reconciled by the existing executionReport
    /// user-data stream — this command only submits the order.
    PlaceOrder {
        #[clap(long, env = "BINANCE_API_KEY", help = "Binance API key")]
        api_key: String,
        #[clap(long, env = "BINANCE_SECRET_KEY", help = "Binance secret key")]
        secret_key: String,
        #[clap(long, help = "Trading pair, e.g. BTCUSDT")]
        symbol: String,
        #[clap(long, help = "BUY or SELL")]
        side: String,
        #[clap(long, help = "Order quantity")]
        quantity: String,
        #[clap(long, help = "Limit price (omit for MARKET order)")]
        price: Option<String>,
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
            Self::PlaceOrder {
                api_key,
                secret_key,
                symbol,
                side,
                quantity,
                price,
            } => {
                use binance_spot_exec::{
                    ApiCredentials, HttpOrderClient, OrderRequest, OrderSide, place_order,
                };

                let credentials = ApiCredentials {
                    api_key: api_key.clone(),
                    secret_key: secret_key.clone(),
                };

                let order_side = match side.parse::<OrderSide>() {
                    Ok(s) => s,
                    Err(e) => {
                        eprintln!("Error: {e}");
                        return;
                    }
                };

                let request = match price {
                    Some(p) => OrderRequest::limit(symbol, order_side, quantity, p),
                    None => OrderRequest::market(symbol, order_side, quantity),
                };

                let client = HttpOrderClient::new();
                match place_order(&client, &credentials, request).await {
                    Ok(ack) => println!(
                        "Order placed: id={} symbol={} status={} client_order_id={}",
                        ack.order_id, ack.symbol, ack.status, ack.client_order_id
                    ),
                    Err(e) => eprintln!("Error placing order: {e}"),
                }
            }
        };
    }
}
