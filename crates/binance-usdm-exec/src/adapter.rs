//! Adapter: translates [`trolly_strategy::OutboundMessage::OrderRequest`] into
//! signed Binance USDM REST orders dispatched to an async worker task.

use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
use trolly_strategy::{OutboundMessage, StreamEgress};

use crate::endpoints::ApiCredentials;
use crate::order::{OrderClient, OrderRequest, OrderSide, place_order};

/// Error type for [`UsdmExecEgress`].
#[derive(Debug)]
pub enum UsdmEgressError {
    /// The order worker task has shut down.
    ChannelClosed,
    /// Strategy emitted an unrecognised order side string.
    InvalidSide(String),
}

impl std::fmt::Display for UsdmEgressError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            UsdmEgressError::ChannelClosed => write!(f, "order worker channel closed"),
            UsdmEgressError::InvalidSide(s) => write!(f, "invalid order side: {s}"),
        }
    }
}

impl std::error::Error for UsdmEgressError {}

/// [`StreamEgress`] implementation that forwards `OrderRequest` messages from
/// a [`trolly_strategy`] runtime to an async order worker via a channel.
///
/// Only [`OutboundMessage::OrderRequest`] messages are acted on; `Subscribe`
/// and `Raw` messages are silently ignored.
///
/// The `position_side` field from [`OutboundMessage::OrderRequest`] is forwarded
/// to the USDM order: set it to `Some("LONG")` / `Some("SHORT")` for hedge mode,
/// or `None` for one-way mode.
pub struct UsdmExecEgress {
    tx: UnboundedSender<OrderRequest>,
}

impl UsdmExecEgress {
    /// Create a new egress adapter together with the paired receiver for the
    /// async order worker.  Spawn [`run_order_worker`] with the returned receiver.
    pub fn new_with_receiver() -> (Self, UnboundedReceiver<OrderRequest>) {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        (Self { tx }, rx)
    }
}

impl StreamEgress for UsdmExecEgress {
    type Error = UsdmEgressError;

    fn dispatch(&mut self, message: OutboundMessage) -> Result<(), Self::Error> {
        let OutboundMessage::OrderRequest {
            symbol,
            side,
            qty,
            price,
            position_side,
        } = message
        else {
            return Ok(());
        };

        let order_side: OrderSide = side
            .parse()
            .map_err(|_| UsdmEgressError::InvalidSide(side.clone()))?;

        let mut request = match price {
            Some(p) => OrderRequest::limit(symbol, order_side, qty, p),
            None => OrderRequest::market(symbol, order_side, qty),
        };

        if let Some(ps) = position_side {
            request = request.with_position_side(ps);
        }

        self.tx
            .send(request)
            .map_err(|_| UsdmEgressError::ChannelClosed)
    }
}

/// Consume orders from `rx` and submit each via `client`.
///
/// Run this as an async task alongside the strategy runtime:
/// ```rust,ignore
/// let (egress, rx) = UsdmExecEgress::new_with_receiver();
/// tokio::spawn(run_order_worker(credentials, HttpOrderClient::new(), rx));
/// ```
///
/// Fills and rejects flow back through the `ORDER_TRADE_UPDATE` user-data
/// stream — this worker only places the order.
pub async fn run_order_worker<C: OrderClient>(
    credentials: ApiCredentials,
    client: C,
    mut rx: UnboundedReceiver<OrderRequest>,
) {
    while let Some(order) = rx.recv().await {
        if let Err(e) = place_order(&client, &credentials, order).await {
            tracing::error!("USDM order placement failed: {e}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::order::MockOrderClient;

    fn make_order_request_msg(price: Option<&str>, position_side: Option<&str>) -> OutboundMessage {
        OutboundMessage::OrderRequest {
            symbol: "BTCUSDT".into(),
            side: "BUY".into(),
            qty: "0.001".into(),
            price: price.map(String::from),
            position_side: position_side.map(String::from),
        }
    }

    #[test]
    fn dispatch_market_order_sends_to_channel() {
        let (mut egress, mut rx) = UsdmExecEgress::new_with_receiver();
        egress.dispatch(make_order_request_msg(None, None)).unwrap();

        let order = rx.try_recv().expect("order must be in channel");
        assert_eq!(order.symbol, "BTCUSDT");
        assert_eq!(order.side, OrderSide::Buy);
        assert!(order.price.is_none());
        assert!(order.position_side.is_none());
    }

    #[test]
    fn dispatch_limit_order_sends_to_channel_with_price() {
        let (mut egress, mut rx) = UsdmExecEgress::new_with_receiver();
        egress
            .dispatch(make_order_request_msg(Some("50000.00"), None))
            .unwrap();

        let order = rx.try_recv().expect("order must be in channel");
        assert_eq!(order.price, Some("50000.00".into()));
    }

    #[test]
    fn dispatch_hedge_mode_order_forwards_position_side() {
        let (mut egress, mut rx) = UsdmExecEgress::new_with_receiver();
        egress
            .dispatch(make_order_request_msg(None, Some("LONG")))
            .unwrap();

        let order = rx.try_recv().expect("order must be in channel");
        assert_eq!(order.position_side, Some("LONG".into()));
    }

    #[test]
    fn dispatch_subscribe_message_is_ignored() {
        let (mut egress, mut rx) = UsdmExecEgress::new_with_receiver();
        egress
            .dispatch(OutboundMessage::Subscribe {
                symbol: "BTCUSDT".into(),
                channel: "depth".into(),
            })
            .unwrap();

        assert!(rx.try_recv().is_err(), "channel must be empty");
    }

    #[test]
    fn dispatch_invalid_side_returns_error() {
        let (mut egress, _rx) = UsdmExecEgress::new_with_receiver();
        let err = egress
            .dispatch(OutboundMessage::OrderRequest {
                symbol: "BTCUSDT".into(),
                side: "LONG".into(),
                qty: "1".into(),
                price: None,
                position_side: Some("LONG".into()),
            })
            .unwrap_err();
        assert!(matches!(err, UsdmEgressError::InvalidSide(_)));
    }

    #[tokio::test]
    async fn run_order_worker_calls_client_for_each_order() {
        use crate::endpoints::ApiCredentials;

        let credentials = ApiCredentials {
            api_key: "key".into(),
            secret_key: "secret".into(),
        };

        let mock = MockOrderClient {
            response: serde_json::json!({
                "symbol": "BTCUSDT",
                "orderId": 1,
                "status": "NEW",
                "clientOrderId": "x"
            }),
        };

        let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<OrderRequest>();
        tx.send(OrderRequest::market("BTCUSDT", OrderSide::Buy, "0.001"))
            .unwrap();
        drop(tx);

        run_order_worker(credentials, mock, rx).await;
    }
}
