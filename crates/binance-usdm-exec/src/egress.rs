use tokio::sync::mpsc;

use trolly_strategy::{OutboundMessage, StreamEgress};

use crate::order::{order_from_outbound, OrderBuildError, PlaceOrderRequest};

#[derive(Debug, Clone, PartialEq)]
pub enum UsdmExecEgressError {
    Build(OrderBuildError),
    QueueClosed,
    Unsupported(String),
}

impl std::fmt::Display for UsdmExecEgressError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Build(err) => write!(f, "{err}"),
            Self::QueueClosed => write!(f, "order queue closed"),
            Self::Unsupported(msg) => write!(f, "unsupported outbound message: {msg}"),
        }
    }
}

impl std::error::Error for UsdmExecEgressError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Build(err) => Some(err),
            _ => None,
        }
    }
}

impl From<OrderBuildError> for UsdmExecEgressError {
    fn from(value: OrderBuildError) -> Self {
        Self::Build(value)
    }
}

/// [`StreamEgress`] adapter that enqueues signed USDM order requests for an async executor.
#[derive(Debug)]
pub struct UsdmExecEgress {
    orders: mpsc::UnboundedSender<PlaceOrderRequest>,
}

impl UsdmExecEgress {
    pub fn new(orders: mpsc::UnboundedSender<PlaceOrderRequest>) -> Self {
        Self { orders }
    }

    pub fn pair() -> (Self, mpsc::UnboundedReceiver<PlaceOrderRequest>) {
        let (tx, rx) = mpsc::unbounded_channel();
        (Self::new(tx), rx)
    }
}

impl StreamEgress for UsdmExecEgress {
    type Error = UsdmExecEgressError;

    fn dispatch(&mut self, message: OutboundMessage) -> Result<(), Self::Error> {
        match message {
            OutboundMessage::OrderRequest {
                symbol,
                side,
                qty,
                price,
                time_in_force,
                position_side,
            } => {
                let order = order_from_outbound(
                    symbol,
                    &side,
                    &qty,
                    price.as_deref(),
                    time_in_force.as_deref(),
                    position_side.as_deref(),
                )?;
                self.orders
                    .send(order)
                    .map_err(|_| UsdmExecEgressError::QueueClosed)
            }
            other => Err(UsdmExecEgressError::Unsupported(format!("{other:?}"))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::order::{OrderType, PositionSide, Side, TimeInForce};

    #[test]
    fn dispatch_queues_limit_order_with_position_side() {
        let (mut egress, mut rx) = UsdmExecEgress::pair();
        egress
            .dispatch(OutboundMessage::OrderRequest {
                symbol: "btcusdt".into(),
                side: "BUY".into(),
                qty: "0.01".into(),
                price: Some("100".into()),
                time_in_force: Some("IOC".into()),
                position_side: Some("LONG".into()),
            })
            .unwrap();

        let order = rx.try_recv().unwrap();
        assert_eq!(order.symbol, "BTCUSDT");
        assert_eq!(order.side, Side::Buy);
        assert_eq!(order.order_type, OrderType::Limit);
        assert_eq!(order.time_in_force, Some(TimeInForce::Ioc));
        assert_eq!(order.position_side, Some(PositionSide::Long));
    }

    #[test]
    fn dispatch_queues_market_order_request() {
        let (mut egress, mut rx) = UsdmExecEgress::pair();
        egress
            .dispatch(OutboundMessage::OrderRequest {
                symbol: "ETHUSDT".into(),
                side: "SELL".into(),
                qty: "1".into(),
                price: None,
                time_in_force: None,
                position_side: None,
            })
            .unwrap();

        let order = rx.try_recv().unwrap();
        assert_eq!(order.order_type, OrderType::Market);
        assert!(order.price.is_none());
        assert!(order.position_side.is_none());
    }

    #[test]
    fn dispatch_rejects_non_order_messages() {
        let (mut egress, _rx) = UsdmExecEgress::pair();
        let msg = OutboundMessage::Subscribe {
            symbol: "BTCUSDT".into(),
            channel: "depth".into(),
        };
        assert!(matches!(
            egress.dispatch(msg),
            Err(UsdmExecEgressError::Unsupported(_))
        ));
    }
}
