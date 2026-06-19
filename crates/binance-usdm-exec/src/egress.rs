use trolly_strategy::{OutboundMessage, StreamEgress};

use crate::client::{OrderError, OrderHttpTransport, UsdmOrderClient};
use crate::order::{NewOrderRequest, new_order_from_outbound};

/// [`StreamEgress`] adapter that places Binance USDM orders via signed REST.
///
/// Order lifecycle updates (fills, rejects) remain on the user-data `ORDER_TRADE_UPDATE` path;
/// this adapter only submits outbound placement requests.
pub struct UsdmOrderEgress<T = crate::client::NativeTlsOrderTransport>
where
    T: OrderHttpTransport,
{
    client: UsdmOrderClient<T>,
    last_response: Option<crate::order::NewOrderResponse>,
}

impl UsdmOrderEgress<crate::client::NativeTlsOrderTransport> {
    pub fn new(credentials: crate::endpoints::ApiCredentials) -> Self {
        Self {
            client: UsdmOrderClient::new(credentials),
            last_response: None,
        }
    }

    pub fn with_base_url(
        credentials: crate::endpoints::ApiCredentials,
        base_url: impl Into<String>,
    ) -> Self {
        Self {
            client: UsdmOrderClient::with_base_url(credentials, base_url),
            last_response: None,
        }
    }
}

impl<T: crate::client::OrderHttpTransport> UsdmOrderEgress<T> {
    pub fn with_client(client: UsdmOrderClient<T>) -> Self {
        Self {
            client,
            last_response: None,
        }
    }

    pub fn client(&self) -> &UsdmOrderClient<T> {
        &self.client
    }

    pub fn last_response(&self) -> Option<&crate::order::NewOrderResponse> {
        self.last_response.as_ref()
    }

    pub fn place_new_order(&mut self, request: &NewOrderRequest) -> Result<(), OrderError> {
        let response = self.client.place_order(request)?;
        self.last_response = Some(response);
        Ok(())
    }

    fn dispatch_order_request(
        &mut self,
        symbol: &str,
        side: &str,
        qty: &str,
        price: Option<&str>,
        time_in_force: Option<&str>,
        position_side: Option<&str>,
    ) -> Result<(), OrderError> {
        let request = new_order_from_outbound(
            symbol,
            side,
            qty,
            price,
            time_in_force,
            position_side,
        )?;
        self.place_new_order(&request)
    }
}

impl<T: crate::client::OrderHttpTransport> StreamEgress for UsdmOrderEgress<T> {
    type Error = OrderError;

    fn dispatch(&mut self, message: OutboundMessage) -> Result<(), Self::Error> {
        match message {
            OutboundMessage::OrderRequest {
                symbol,
                side,
                qty,
                price,
                time_in_force,
                position_side,
            } => self.dispatch_order_request(
                &symbol,
                &side,
                &qty,
                price.as_deref(),
                time_in_force.as_deref(),
                position_side.as_deref(),
            ),
            OutboundMessage::Subscribe { symbol, channel } => Err(OrderError::UnsupportedEgress(
                format!("Subscribe {{ symbol: {symbol}, channel: {channel} }}"),
            )),
            OutboundMessage::Raw(_) => {
                Err(OrderError::UnsupportedEgress("Raw websocket frame".into()))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    use crate::client::MockOrderTransport;
    use crate::endpoints::ApiCredentials;
    use trolly_strategy::{DepthUpdate, PriceLevel, RecordingStrategy, StreamEvent, StrategyHub};

    const MOCK_ACK: &str = r#"{
        "symbol": "BTCUSDT",
        "orderId": 99,
        "clientOrderId": "cli-99",
        "updateTime": 1507725176595,
        "price": "100.00",
        "origQty": "0.01",
        "executedQty": "0.00",
        "status": "NEW",
        "timeInForce": "GTC",
        "type": "LIMIT",
        "side": "BUY",
        "positionSide": "LONG"
    }"#;

    #[test]
    fn egress_dispatches_limit_order_request_with_position_side() {
        let transport = MockOrderTransport::with_response(MOCK_ACK.as_bytes());
        let requests = Arc::clone(&transport.requests);
        let client = UsdmOrderClient::with_transport(
            ApiCredentials {
                api_key: "key".into(),
                secret_key: "secret".into(),
            },
            "https://fapi.binance.com",
            transport,
        );
        let mut egress = UsdmOrderEgress::with_client(client);

        egress
            .dispatch(
                OutboundMessage::limit_order("BTCUSDT", "BUY", "0.01", "100", Some("GTC"))
                    .with_position_side("LONG"),
            )
            .unwrap();

        let response = egress.last_response().unwrap();
        assert_eq!(response.order_id, 99);
        assert_eq!(response.status, "NEW");
        assert_eq!(response.position_side.as_deref(), Some("LONG"));

        let body = &requests.lock().expect("mock lock")[0].2;
        assert!(body.contains("type=LIMIT"));
        assert!(body.contains("price=100"));
        assert!(body.contains("positionSide=LONG"));
    }

    #[test]
    fn strategy_runtime_can_drive_usdm_order_egress() {
        let transport = MockOrderTransport::with_response(MOCK_ACK.as_bytes());
        let egress = UsdmOrderEgress::with_client(UsdmOrderClient::with_transport(
            ApiCredentials {
                api_key: "key".into(),
                secret_key: "secret".into(),
            },
            "https://fapi.binance.com",
            transport,
        ));
        let outbound = OutboundMessage::market_order("BTCUSDT", "SELL", "1")
            .with_position_side("SHORT");
        let mut hub = StrategyHub::new(
            RecordingStrategy::with_responses(vec![outbound.clone()]),
            egress,
        );

        hub.ingest_event(StreamEvent::Depth(DepthUpdate {
            symbol: "BTCUSDT".into(),
            bids: vec![PriceLevel {
                price: "100".into(),
                qty: "1".into(),
            }],
            asks: vec![],
            update_id: None,
        }))
        .unwrap();

        let response = hub.runtime().egress().last_response().unwrap();
        assert_eq!(response.symbol, "BTCUSDT");
        assert_eq!(hub.runtime().strategy().consumed.len(), 1);
    }

    #[test]
    fn egress_rejects_non_order_messages() {
        let transport = MockOrderTransport::with_response(MOCK_ACK.as_bytes());
        let mut egress = UsdmOrderEgress::with_client(UsdmOrderClient::with_transport(
            ApiCredentials {
                api_key: "k".into(),
                secret_key: "s".into(),
            },
            "https://fapi.binance.com",
            transport,
        ));
        let err = egress
            .dispatch(OutboundMessage::Subscribe {
                symbol: "BTCUSDT".into(),
                channel: "depth".into(),
            })
            .unwrap_err();
        assert!(matches!(err, OrderError::UnsupportedEgress(_)));
    }
}
