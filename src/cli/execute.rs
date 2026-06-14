//! Execute subcommands: minimal signed spot order placement.

use std::env;

use binance_spot_exec::{
    ApiCredentials, NewOrderRequest, OrderSide, SpotOrderClient, TimeInForce,
};
use clap::{Args, Subcommand};
use tracing::info;

#[derive(Subcommand, Debug, Clone)]
pub enum ExecuteCommands {
    /// Place a signed spot order via REST (`POST /api/v3/order`).
    PlaceOrder(PlaceOrderArgs),
}

#[derive(Args, Debug, Clone)]
pub struct PlaceOrderArgs {
    /// Trading pair, e.g. BTCUSDT.
    pub symbol: String,
    /// BUY or SELL.
    pub side: String,
    /// Order quantity.
    pub quantity: String,
    /// Limit price. Omit for market orders.
    #[arg(long)]
    pub price: Option<String>,
    /// Time in force for limit orders (GTC, IOC, FOK). Defaults to GTC.
    #[arg(long, default_value = "GTC")]
    pub time_in_force: String,
    /// Use Binance spot demo REST host (`https://demo-api.binance.com`).
    #[arg(long)]
    pub demo: bool,
}

impl ExecuteCommands {
    pub async fn run(self) -> Result<(), color_eyre::Report> {
        match self {
            Self::PlaceOrder(args) => args.run().await,
        }
    }
}

impl PlaceOrderArgs {
    pub async fn run(self) -> Result<(), color_eyre::Report> {
        let api_key = env::var("DEMO_BINANCE_KEY")
            .or_else(|_| env::var("BINANCE_API_KEY"))
            .map_err(|_| {
                color_eyre::eyre::eyre!(
                    "missing API key: set DEMO_BINANCE_KEY or BINANCE_API_KEY"
                )
            })?;
        let secret_key = env::var("DEMO_BINANCE_SECRET")
            .or_else(|_| env::var("BINANCE_API_SECRET"))
            .map_err(|_| {
                color_eyre::eyre::eyre!(
                    "missing API secret: set DEMO_BINANCE_SECRET or BINANCE_API_SECRET"
                )
            })?;

        let side = OrderSide::parse(&self.side).map_err(color_eyre::Report::from)?;
        let request = match self.price {
            Some(price) => {
                let tif = TimeInForce::parse(&self.time_in_force)
                    .map_err(color_eyre::Report::from)?;
                NewOrderRequest::limit(
                    &self.symbol,
                    side,
                    &self.quantity,
                    price,
                    tif,
                )
            }
            None => NewOrderRequest::market(&self.symbol, side, &self.quantity),
        };

        let client = if self.demo {
            SpotOrderClient::demo(ApiCredentials {
                api_key,
                secret_key,
            })
        } else {
            SpotOrderClient::new(ApiCredentials {
                api_key,
                secret_key,
            })
        };

        info!(
            symbol = %request.symbol,
            side = ?request.side,
            order_type = ?request.order_type,
            rest_base = %client.rest_base(),
            "placing spot order"
        );

        let ack = client.place_order(&request).await?;
        info!(
            order_id = ack.order_id,
            status = %ack.status,
            executed_qty = %ack.executed_qty,
            "order acknowledged; reconcile fills via user-data executionReport stream"
        );
        println!("{}", serde_json::to_string_pretty(&ack)?);
        Ok(())
    }
}
