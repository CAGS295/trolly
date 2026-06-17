use tokio::sync::mpsc;

use trolly_strategy::{OutboundMessage, StreamEgress};

use crate::order::{order_from_outbound, OrderBuildError, PlaceOrderRequest};

#[derive(Debug, Clone, PartialEq)]
pub enum SpotExecEgressError {
    Build(OrderBuildError),
    QueueClosed,
    Unsupported(String),
}

impl std::fmt::Display for SpotExecEgressError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Build(err) => write!(f, "{err}"),
            Self::QueueClosed => write!(f, "order queue closed"),
            Self::Unsupported(msg) => write!(f, "unsupported outbound message: {msg}"),
        }
    }
}

impl std::error::Error for SpotExecEgressError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Build(err) => Some(err),
            _ => None,
        }
    }
}

impl From<OrderBuildError> for SpotExecEgressError {
    fn from(value: OrderBuildError) -> Self {
        Self::Build(value)
    }
}

/// [`StreamEgress`] adapter that enqueues signed spot order requests for an async executor.
#[derive(Debug)]
pub struct SpotExecEgress {
    orders: mpsc::UnboundedSender<PlaceOrderRequest>,
}

impl SpotExecEgress {
    pub fn new(orders: mpsc::UnboundedSender<PlaceOrderRequest>) -> Self {
        Self { orders }
    }

    pub fn pair() -> (Self, mpsc::UnboundedReceiver<PlaceOrderRequest>) {
        let (tx, rx) = mpsc::unbounded_channel();
        (Self::new(tx), rx)
    }
}

impl StreamEgress for SpotExecEgress {
    type Error = SpotExecEgressError;

    fn dispatch(&mut self, message: OutboundMessage) -> Result<(), Self::Error> {
        match message {
            OutboundMessage::OrderRequest {
                symbol,
                side,
                qty,
                price,
                time_in_force,
            } => {
                let order = order_from_outbound(
                    symbol,
                    &side,
                    &qty,
                    price.as_deref(),
                    time_in_force.as_deref(),
                )?;
                self.orders
                    .send(order)
                    .map_err(|_| SpotExecEgressError::QueueClosed)
            }
            other => Err(SpotExecEgressError::Unsupported(format!("{other:?}"))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::order::{OrderType, Side, TimeInForce};

    #[test]
    fn dispatch_queues_limit_order_request() {
        let (mut egress, mut rx) = SpotExecEgress::pair();
        egress
            .dispatch(OutboundMessage::OrderRequest {
                symbol: "btcusdt".into(),
                side: "BUY".into(),
                qty: "0.01".into(),
                price: Some("100".into()),
                time_in_force: Some("IOC".into()),
            })
            .unwrap();

        let order = rx.try_recv().unwrap();
        assert_eq!(order.symbol, "BTCUSDT");
        assert_eq!(order.side, Side::Buy);
        assert_eq!(order.order_type, OrderType::Limit);
        assert_eq!(order.time_in_force, Some(TimeInForce::Ioc));
    }

    #[test]
    fn dispatch_queues_market_order_request() {
        let (mut egress, mut rx) = SpotExecEgress::pair();
        egress
            .dispatch(OutboundMessage::OrderRequest {
                symbol: "ETHUSDT".into(),
                side: "SELL".into(),
                qty: "1".into(),
                price: None,
                time_in_force: None,
            })
            .unwrap();

        let order = rx.try_recv().unwrap();
        assert_eq!(order.order_type, OrderType::Market);
        assert!(order.price.is_none());
    }

    #[test]
    fn dispatch_rejects_non_order_messages() {
        let (mut egress, _rx) = SpotExecEgress::pair();
        let msg = OutboundMessage::Subscribe {
            symbol: "BTCUSDT".into(),
            channel: "depth".into(),
        };
        assert!(matches!(
            egress.dispatch(msg),
            Err(SpotExecEgressError::Unsupported(_))
        ));
    }
}
