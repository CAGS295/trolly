use trolly_strategy::{OutboundMessage, StreamEgress};

use crate::endpoints::ApiCredentials;
use crate::order::{
    parse_order_side, parse_position_side, OrderError, UsdmOrderClient, UsdmOrderRequest,
    UsdmOrderResponse,
};

fn blocking_runtime() -> &'static tokio::runtime::Runtime {
    static RUNTIME: std::sync::OnceLock<tokio::runtime::Runtime> = std::sync::OnceLock::new();
    RUNTIME.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("usdm egress blocking runtime")
    })
}

/// Dispatches [`OutboundMessage::OrderRequest`] commands as signed REST `POST /fapi/v1/order` calls.
///
/// Fills and rejects are still reconciled through the existing user-data `ORDER_TRADE_UPDATE` path;
/// this adapter only performs outbound placement.
#[derive(Clone, Debug)]
pub struct UsdmRestEgress {
    client: UsdmOrderClient,
}

impl UsdmRestEgress {
    pub fn new(base_url: impl Into<String>, credentials: ApiCredentials) -> Self {
        Self {
            client: UsdmOrderClient::new(base_url, credentials),
        }
    }

    pub fn with_client(client: UsdmOrderClient) -> Self {
        Self { client }
    }

    pub fn client(&self) -> &UsdmOrderClient {
        &self.client
    }

    fn order_request_from_outbound(
        symbol: String,
        side: String,
        qty: String,
        price: Option<String>,
        position_side: Option<String>,
    ) -> Result<UsdmOrderRequest, OrderError> {
        let side = parse_order_side(&side)?;
        let position_side = match position_side {
            Some(value) => Some(parse_position_side(&value)?),
            None => None,
        };

        let mut request = match price {
            Some(price) => UsdmOrderRequest::limit(symbol, side, qty, price),
            None => UsdmOrderRequest::market(symbol, side, qty),
        };
        request.position_side = position_side;
        Ok(request)
    }

    pub async fn dispatch_async(
        &self,
        message: OutboundMessage,
    ) -> Result<Option<UsdmOrderResponse>, OrderError> {
        match message {
            OutboundMessage::OrderRequest {
                symbol,
                side,
                qty,
                price,
                position_side,
            } => {
                let request =
                    Self::order_request_from_outbound(symbol, side, qty, price, position_side)?;
                let response = self.client.place_order(request).await?;
                Ok(Some(response))
            }
            OutboundMessage::Subscribe { .. } | OutboundMessage::Raw(_) => Ok(None),
        }
    }
}

impl StreamEgress for UsdmRestEgress {
    type Error = OrderError;

    fn dispatch(&mut self, message: OutboundMessage) -> Result<(), Self::Error> {
        let egress = self.clone();
        std::thread::spawn(move || {
            blocking_runtime()
                .block_on(egress.dispatch_async(message))
                .map(|_| ())
        })
        .join()
        .expect("usdm egress worker thread panicked")
    }
}
