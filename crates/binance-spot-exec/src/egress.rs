//! Bridge from [`trolly_strategy::OutboundMessage`] to signed spot order placement.

use trolly_strategy::{OutboundMessage, StreamEgress};

use crate::endpoints::ApiCredentials;
use crate::order::{NewOrderRequest, SpotOrderClient, SpotOrderError};

/// Dispatches strategy [`OutboundMessage::OrderRequest`] commands via REST placement.
///
/// Fills and rejects are reconciled by the existing user-data `executionReport` path;
/// this adapter does not maintain a separate order state machine.
#[derive(Clone, Debug)]
pub struct SpotOrderEgress {
    client: SpotOrderClient,
}

impl SpotOrderEgress {
    pub fn new(credentials: ApiCredentials) -> Self {
        Self {
            client: SpotOrderClient::new(credentials),
        }
    }

    pub fn demo(credentials: ApiCredentials) -> Self {
        Self {
            client: SpotOrderClient::demo(credentials),
        }
    }

    pub fn with_client(client: SpotOrderClient) -> Self {
        Self { client }
    }

    pub fn client(&self) -> &SpotOrderClient {
        &self.client
    }
}

impl StreamEgress for SpotOrderEgress {
    type Error = SpotOrderError;

    fn dispatch(&mut self, message: OutboundMessage) -> Result<(), Self::Error> {
        match message {
            OutboundMessage::OrderRequest {
                symbol,
                side,
                qty,
                price,
            } => {
                let request = NewOrderRequest::from_strategy_order(symbol, &side, qty, price)?;
                self.client.place_order_blocking(&request)?;
                Ok(())
            }
            other => Err(SpotOrderError::UnsupportedOutbound(other)),
        }
    }
}