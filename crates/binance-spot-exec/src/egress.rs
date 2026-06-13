//! [`trolly_strategy::StreamEgress`] adapter for spot order placement.

use std::sync::Arc;

use thiserror::Error;
use tokio::sync::mpsc;
use trolly_strategy::{OutboundMessage, StreamEgress};

use crate::endpoints::ApiCredentials;
use crate::order::{
    OrderSide, OrderTransport, OrderType, PlaceOrderError, PlaceOrderRequest, SpotOrderClient,
    TimeInForce,
};

#[derive(Debug, Error)]
pub enum EgressError {
    #[error(transparent)]
    Builder(#[from] crate::order::OrderBuilderError),
    #[error("unsupported outbound message: {0}")]
    Unsupported(&'static str),
    #[error("order channel closed")]
    ChannelClosed,
    #[error(transparent)]
    PlaceOrder(#[from] PlaceOrderError),
}

/// Convert a normalized strategy command into a spot place-order request.
pub fn place_order_from_outbound(message: OutboundMessage) -> Result<PlaceOrderRequest, EgressError> {
    match message {
        OutboundMessage::OrderRequest {
            symbol,
            side,
            qty,
            price,
            time_in_force,
            position_side: _,
        } => {
            let side = OrderSide::parse(&side)?;
            let order_type = if price.is_some() {
                OrderType::Limit
            } else {
                OrderType::Market
            };
            let time_in_force = time_in_force
                .map(|tif| TimeInForce::parse(&tif))
                .transpose()?;
            Ok(PlaceOrderRequest {
                symbol: symbol.to_ascii_uppercase(),
                side,
                order_type,
                quantity: qty,
                price,
                time_in_force,
                new_client_order_id: None,
            })
        }
        OutboundMessage::Subscribe { .. } => Err(EgressError::Unsupported("subscribe")),
        OutboundMessage::Raw(_) => Err(EgressError::Unsupported("raw")),
    }
}

/// Queues place-order requests for an async executor task.
#[derive(Debug)]
pub struct SpotOrderEgress {
    tx: mpsc::UnboundedSender<PlaceOrderRequest>,
}

impl SpotOrderEgress {
    pub fn new(tx: mpsc::UnboundedSender<PlaceOrderRequest>) -> Self {
        Self { tx }
    }

    pub fn channel() -> (Self, mpsc::UnboundedReceiver<PlaceOrderRequest>) {
        let (tx, rx) = mpsc::unbounded_channel();
        (Self::new(tx), rx)
    }
}

impl StreamEgress for SpotOrderEgress {
    type Error = EgressError;

    fn dispatch(&mut self, message: OutboundMessage) -> Result<(), Self::Error> {
        let request = place_order_from_outbound(message)?;
        self.tx
            .send(request)
            .map_err(|_| EgressError::ChannelClosed)
    }
}

/// Direct egress that places orders synchronously via an async client call.
///
/// Intended for CLI and tests with a mock transport; uses the current tokio runtime.
#[derive(Clone)]
pub struct SpotOrderEgressDirect<T: OrderTransport> {
    client: Arc<SpotOrderClient<T>>,
}

impl<T: OrderTransport + 'static> SpotOrderEgressDirect<T> {
    pub fn new(credentials: ApiCredentials, transport: T) -> Self {
        Self {
            client: Arc::new(SpotOrderClient::new(credentials, transport)),
        }
    }

    pub fn client(&self) -> &SpotOrderClient<T> {
        &self.client
    }
}

impl<T: OrderTransport + Send + Sync + 'static> StreamEgress for SpotOrderEgressDirect<T> {
    type Error = EgressError;

    fn dispatch(&mut self, message: OutboundMessage) -> Result<(), Self::Error> {
        let request = place_order_from_outbound(message)?;
        let client = Arc::clone(&self.client);
        tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(async move {
                client.place_order(request).await?;
                Ok::<(), EgressError>(())
            })
        })
    }
}

/// Drain queued requests and place them via REST.
pub async fn run_order_executor<T: OrderTransport + 'static>(
    client: SpotOrderClient<T>,
    mut rx: mpsc::UnboundedReceiver<PlaceOrderRequest>,
) -> Result<(), PlaceOrderError> {
    while let Some(request) = rx.recv().await {
        client.place_order(request).await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::order::{HttpResponse, OrderSide, OrderType};
    use crate::order::mock_transport::MockTransport;
    use trolly_strategy::{StrategyHub};
    use trolly_strategy::{DepthUpdate, PriceLevel, RecordingStrategy, StreamEvent};

    #[test]
    fn outbound_market_order_maps_to_place_request() {
        let request = place_order_from_outbound(OutboundMessage::OrderRequest {
            symbol: "btcusdt".into(),
            side: "BUY".into(),
            qty: "0.01".into(),
            price: None,
            time_in_force: None,
            position_side: None,
        })
        .unwrap();
        assert_eq!(request.symbol, "BTCUSDT");
        assert_eq!(request.side, OrderSide::Buy);
        assert_eq!(request.order_type, OrderType::Market);
        assert_eq!(request.quantity, "0.01");
    }

    #[test]
    fn outbound_limit_order_maps_price_and_tif() {
        let request = place_order_from_outbound(OutboundMessage::OrderRequest {
            symbol: "ETHUSDT".into(),
            side: "SELL".into(),
            qty: "1".into(),
            price: Some("3000".into()),
            time_in_force: Some("IOC".into()),
            position_side: None,
        })
        .unwrap();
        assert_eq!(request.order_type, OrderType::Limit);
        assert_eq!(request.price, Some("3000".into()));
        assert_eq!(request.time_in_force, Some(TimeInForce::Ioc));
    }

    #[test]
    fn spot_order_egress_enqueues_request() {
        let (mut egress, mut rx) = SpotOrderEgress::channel();
        egress
            .dispatch(OutboundMessage::OrderRequest {
                symbol: "BTCUSDT".into(),
                side: "BUY".into(),
                qty: "0.01".into(),
                price: Some("100".into()),
                time_in_force: None,
                position_side: None,
            })
            .unwrap();

        let queued = rx.try_recv().unwrap();
        assert_eq!(queued.symbol, "BTCUSDT");
        assert_eq!(queued.order_type, OrderType::Limit);
    }

    #[test]
    fn strategy_runtime_dispatches_to_spot_egress() {
        let outbound = OutboundMessage::OrderRequest {
            symbol: "BTCUSDT".into(),
            side: "BUY".into(),
            qty: "0.01".into(),
            price: Some("100".into()),
            time_in_force: Some("GTC".into()),
            position_side: None,
        };
        let (egress, mut rx) = SpotOrderEgress::channel();
        let mut hub = StrategyHub::new(
            RecordingStrategy::with_responses(vec![outbound]),
            egress,
        );

        hub.ingest_event(StreamEvent::Depth(DepthUpdate {
            symbol: "BTCUSDT".into(),
            bids: vec![PriceLevel {
                price: "99".into(),
                qty: "1".into(),
            }],
            asks: vec![],
            update_id: Some(1),
        }))
        .unwrap();

        let queued = rx.try_recv().unwrap();
        assert_eq!(queued.symbol, "BTCUSDT");
        assert_eq!(queued.side, OrderSide::Buy);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn direct_egress_places_order_via_mock_transport() {
        let transport = MockTransport::with_response(HttpResponse {
            status: 200,
            body: include_str!("../tests/fixtures/place_order_ack.json").into(),
        });
        let credentials = ApiCredentials {
            api_key: "key".into(),
            secret_key: "secret".into(),
        };
        let mut egress = SpotOrderEgressDirect::new(credentials, transport.clone());

        egress
            .dispatch(OutboundMessage::OrderRequest {
                symbol: "BTCUSDT".into(),
                side: "BUY".into(),
                qty: "0.01".into(),
                price: Some("50000".into()),
                time_in_force: None,
                position_side: None,
            })
            .unwrap();

        assert_eq!(transport.captured.lock().unwrap().len(), 1);
    }
}
