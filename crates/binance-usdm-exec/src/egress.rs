//! Bridge from [`trolly_strategy::OutboundMessage`] to signed USDM order placement.

use trolly_strategy::{OutboundMessage, StreamEgress};

use crate::endpoints::ApiCredentials;
use crate::order::{NewOrderRequest, UsdmOrderClient, UsdmOrderError};

/// Dispatches strategy [`OutboundMessage::OrderRequest`] commands via REST placement.
///
/// Fills and rejects are reconciled by the existing user-data `ORDER_TRADE_UPDATE` path;
/// this adapter does not maintain a separate order state machine.
#[derive(Clone, Debug)]
pub struct UsdmOrderEgress {
    client: UsdmOrderClient,
}

impl UsdmOrderEgress {
    pub fn new(credentials: ApiCredentials) -> Self {
        Self {
            client: UsdmOrderClient::new(credentials),
        }
    }

    pub fn demo(credentials: ApiCredentials) -> Self {
        Self {
            client: UsdmOrderClient::demo(credentials),
        }
    }

    pub fn with_client(client: UsdmOrderClient) -> Self {
        Self { client }
    }

    pub fn client(&self) -> &UsdmOrderClient {
        &self.client
    }
}

impl StreamEgress for UsdmOrderEgress {
    type Error = UsdmOrderError;

    fn dispatch(&mut self, message: OutboundMessage) -> Result<(), Self::Error> {
        match message {
            OutboundMessage::OrderRequest {
                symbol,
                side,
                qty,
                price,
                position_side,
            } => {
                let request =
                    NewOrderRequest::from_strategy_order(symbol, &side, qty, price, position_side)?;
                self.client.place_order_blocking(&request)?;
                Ok(())
            }
            other => Err(UsdmOrderError::UnsupportedOutbound(other)),
        }
    }
}
