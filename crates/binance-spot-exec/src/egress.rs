use trolly_strategy::{OutboundMessage, StreamEgress};

use crate::endpoints::ApiCredentials;
use crate::order::{parse_order_side, OrderError, SpotOrderClient, SpotOrderRequest, SpotOrderResponse};

fn blocking_runtime() -> &'static tokio::runtime::Runtime {
    static RUNTIME: std::sync::OnceLock<tokio::runtime::Runtime> = std::sync::OnceLock::new();
    RUNTIME.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("spot egress blocking runtime")
    })
}

/// Dispatches [`OutboundMessage::OrderRequest`] commands as signed REST `POST /api/v3/order` calls.
///
/// Fills and rejects are still reconciled through the existing user-data `executionReport` path;
/// this adapter only performs outbound placement.
#[derive(Clone, Debug)]
pub struct SpotRestEgress {
    client: SpotOrderClient,
}

impl SpotRestEgress {
    pub fn new(base_url: impl Into<String>, credentials: ApiCredentials) -> Self {
        Self {
            client: SpotOrderClient::new(base_url, credentials),
        }
    }

    pub fn with_client(client: SpotOrderClient) -> Self {
        Self { client }
    }

    pub fn client(&self) -> &SpotOrderClient {
        &self.client
    }

    fn order_request_from_outbound(
        symbol: String,
        side: String,
        qty: String,
        price: Option<String>,
    ) -> Result<SpotOrderRequest, OrderError> {
        let side = parse_order_side(&side)?;
        Ok(match price {
            Some(price) => SpotOrderRequest::limit(symbol, side, qty, price),
            None => SpotOrderRequest::market(symbol, side, qty),
        })
    }

    pub async fn dispatch_async(
        &self,
        message: OutboundMessage,
    ) -> Result<Option<SpotOrderResponse>, OrderError> {
        match message {
            OutboundMessage::OrderRequest {
                symbol,
                side,
                qty,
                price,
            } => {
                let request = Self::order_request_from_outbound(symbol, side, qty, price)?;
                let response = self.client.place_order(request).await?;
                Ok(Some(response))
            }
            OutboundMessage::Subscribe { .. } | OutboundMessage::Raw(_) => Ok(None),
        }
    }
}

impl StreamEgress for SpotRestEgress {
    type Error = OrderError;

    fn dispatch(&mut self, message: OutboundMessage) -> Result<(), Self::Error> {
        let egress = self.clone();
        std::thread::spawn(move || {
            blocking_runtime()
                .block_on(egress.dispatch_async(message))
                .map(|_| ())
        })
        .join()
        .expect("spot egress worker thread panicked")
    }
}
