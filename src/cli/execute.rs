//! Minimal spot order placement CLI (`trolly execute spot-order`).

use clap::Args;

use binance_spot_exec::{
    ApiCredentials, NewOrderRequest, OrderBuildError, OrderSide, SpotOrderClient, TimeInForce,
};

#[derive(Args, Debug, Clone)]
pub struct SpotOrderArgs {
    /// Trading pair, e.g. BTCUSDT
    #[arg(long)]
    pub symbol: String,
    /// BUY or SELL
    #[arg(long)]
    pub side: String,
    /// Order quantity
    #[arg(long)]
    pub qty: String,
    /// Limit price (omit for market order)
    #[arg(long)]
    pub price: Option<String>,
    /// Limit time-in-force: GTC, IOC, or FOK (default GTC)
    #[arg(long, default_value = "GTC")]
    pub time_in_force: String,
}

impl SpotOrderArgs {
    pub fn build_request(&self) -> Result<NewOrderRequest, OrderBuildError> {
        let side = OrderSide::parse(&self.side)?;
        match &self.price {
            Some(price) => {
                let tif = TimeInForce::parse(&self.time_in_force)?;
                Ok(NewOrderRequest::limit(
                    &self.symbol,
                    side,
                    &self.qty,
                    price,
                    tif,
                ))
            }
            None => Ok(NewOrderRequest::market(&self.symbol, side, &self.qty)),
        }
    }

    pub async fn run(self) {
        let api_key = std::env::var("DEMO_BINANCE_KEY")
            .expect("DEMO_BINANCE_KEY must be set for signed spot order placement");
        let secret_key = std::env::var("DEMO_BINANCE_SECRET")
            .expect("DEMO_BINANCE_SECRET must be set for signed spot order placement");

        let request = self
            .build_request()
            .unwrap_or_else(|err| panic!("invalid order arguments: {err}"));

        let client = SpotOrderClient::new(ApiCredentials {
            api_key,
            secret_key,
        });

        match tokio::task::spawn_blocking(move || client.place_order(&request)).await {
            Ok(Ok(response)) => {
                println!(
                    "order placed: id={} symbol={} status={} side={} type={}",
                    response.order_id,
                    response.symbol,
                    response.status,
                    response.side,
                    response.order_type
                );
                println!(
                    "note: fills/rejects reconcile via user-data executionReport stream"
                );
            }
            Ok(Err(err)) => panic!("order placement failed: {err}"),
            Err(join_err) => panic!("order task failed: {join_err}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use binance_spot_exec::{MockOrderTransport, SpotOrderClient};

    const MOCK_ACK: &str = r#"{
        "symbol": "BTCUSDT",
        "orderId": 1,
        "clientOrderId": "x",
        "transactTime": 1,
        "price": "100",
        "origQty": "0.01",
        "executedQty": "0",
        "status": "NEW",
        "timeInForce": "GTC",
        "type": "LIMIT",
        "side": "BUY"
    }"#;

    #[test]
    fn spot_order_args_build_limit_request() {
        let args = SpotOrderArgs {
            symbol: "BTCUSDT".into(),
            side: "buy".into(),
            qty: "0.01".into(),
            price: Some("100".into()),
            time_in_force: "IOC".into(),
        };
        let request = args.build_request().unwrap();
        assert_eq!(request.order_type, binance_spot_exec::OrderType::Limit);
        assert_eq!(request.time_in_force, Some(TimeInForce::Ioc));
    }

    #[test]
    fn spot_order_args_build_market_request() {
        let args = SpotOrderArgs {
            symbol: "ETHUSDT".into(),
            side: "SELL".into(),
            qty: "1".into(),
            price: None,
            time_in_force: "GTC".into(),
        };
        let request = args.build_request().unwrap();
        assert_eq!(request.order_type, binance_spot_exec::OrderType::Market);
    }

    #[test]
    fn mock_transport_place_order_from_built_request() {
        let transport = MockOrderTransport::with_response(MOCK_ACK.as_bytes());
        let client = SpotOrderClient::with_transport(
            ApiCredentials {
                api_key: "k".into(),
                secret_key: "s".into(),
            },
            "https://api.binance.com",
            transport,
        );
        let request = SpotOrderArgs {
            symbol: "BTCUSDT".into(),
            side: "BUY".into(),
            qty: "0.01".into(),
            price: Some("100".into()),
            time_in_force: "GTC".into(),
        }
        .build_request()
        .unwrap();
        let response = client.place_order(&request).unwrap();
        assert_eq!(response.order_id, 1);
        assert_eq!(response.status, "NEW");
    }
}
