//! Signed outbound spot order placement via REST and strategy egress adapter.

use std::sync::Arc;

use reqwest::header::{HeaderMap, HeaderValue, CONTENT_TYPE};
use reqwest::Client;
use serde::Deserialize;
use trolly_strategy::{OutboundMessage, StreamEgress};
use tokio::runtime::Runtime;

use crate::auth::signed_params_payload;
use crate::endpoints::{ApiCredentials, BinanceSpotRest};
use crate::order::{NewOrderRequest, OrderBuildError, OrderSide, TimeInForce};

/// Minimal REST acknowledgement; fills/rejects arrive on user-data `executionReport`.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct PlaceOrderResponse {
    #[serde(rename = "orderId")]
    pub order_id: i64,
    #[serde(rename = "clientOrderId")]
    pub client_order_id: String,
    pub symbol: String,
    pub status: String,
}

#[derive(Debug)]
pub enum OrderError {
    Build(OrderBuildError),
    Http(reqwest::Error),
    Api { status: u16, body: String },
}

impl std::fmt::Display for OrderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Build(e) => write!(f, "{e}"),
            Self::Http(e) => write!(f, "http error: {e}"),
            Self::Api { status, body } => write!(f, "binance api error ({status}): {body}"),
        }
    }
}

impl std::error::Error for OrderError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Build(e) => Some(e),
            Self::Http(e) => Some(e),
            Self::Api { .. } => None,
        }
    }
}

impl From<OrderBuildError> for OrderError {
    fn from(value: OrderBuildError) -> Self {
        Self::Build(value)
    }
}

/// Async REST client for `POST /api/v3/order`.
#[derive(Clone)]
pub struct SpotOrderClient {
    http: Client,
    base_url: String,
    credentials: ApiCredentials,
}

impl SpotOrderClient {
    pub fn new(credentials: ApiCredentials) -> Self {
        Self::with_base_url(BinanceSpotRest::PRODUCTION_URL, credentials)
    }

    pub fn with_base_url(base_url: impl Into<String>, credentials: ApiCredentials) -> Self {
        Self {
            http: Client::new(),
            base_url: base_url.into(),
            credentials,
        }
    }

    pub fn credentials(&self) -> &ApiCredentials {
        &self.credentials
    }

    pub async fn place_order(
        &self,
        order: &NewOrderRequest,
    ) -> Result<PlaceOrderResponse, OrderError> {
        let params = order.to_signed_params(&self.credentials.secret_key)?;
        let query = signed_params_payload(&params);
        let url = format!("{}/api/v3/order?{query}", self.base_url.trim_end_matches('/'));

        let mut headers = HeaderMap::new();
        headers.insert(
            "X-MBX-APIKEY",
            HeaderValue::from_str(&self.credentials.api_key)
                .expect("api key must be a valid header value"),
        );
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/x-www-form-urlencoded"));

        let response = self
            .http
            .post(&url)
            .headers(headers)
            .send()
            .await
            .map_err(OrderError::Http)?;

        let status = response.status();
        let body = response.text().await.map_err(OrderError::Http)?;
        if !status.is_success() {
            return Err(OrderError::Api {
                status: status.as_u16(),
                body,
            });
        }

        serde_json::from_str(&body).map_err(|e| OrderError::Api {
            status: status.as_u16(),
            body: format!("invalid json: {e}; body={body}"),
        })
    }
}

/// Convenience builder used by the CLI and tests.
pub fn build_order(
    symbol: &str,
    side: &str,
    quantity: &str,
    price: Option<&str>,
    time_in_force: Option<&str>,
) -> Result<NewOrderRequest, OrderBuildError> {
    let side = OrderSide::parse(side)?;
    if let Some(price) = price {
        let tif = time_in_force
            .map(TimeInForce::parse)
            .transpose()?
            .unwrap_or_default();
        Ok(NewOrderRequest::limit(symbol, side, quantity, price, tif))
    } else {
        Ok(NewOrderRequest::market(symbol, side, quantity))
    }
}

/// [`StreamEgress`] adapter: dispatches [`OutboundMessage::OrderRequest`] via REST.
///
/// Order lifecycle (fills, rejects, cancels) is reconciled through the existing
/// user-data `executionReport` ingest path — no duplicate state machine here.
pub struct SpotExecEgress {
    client: Arc<SpotOrderClient>,
    runtime: Runtime,
}

impl SpotExecEgress {
    pub fn new(client: SpotOrderClient) -> Self {
        Self {
            client: Arc::new(client),
            runtime: Runtime::new().expect("tokio runtime"),
        }
    }

    pub fn client(&self) -> &SpotOrderClient {
        &self.client
    }
}

impl StreamEgress for SpotExecEgress {
    type Error = OrderError;

    fn dispatch(&mut self, message: OutboundMessage) -> Result<(), Self::Error> {
        match message {
            OutboundMessage::OrderRequest { .. } => {
                let order = NewOrderRequest::try_from(&message)?;
                let client = Arc::clone(&self.client);
                if let Ok(handle) = tokio::runtime::Handle::try_current() {
                    tokio::task::block_in_place(|| {
                        handle.block_on(async move { client.place_order(&order).await })
                    })
                } else {
                    self.runtime
                        .block_on(async move { client.place_order(&order).await })
                }?;
                Ok(())
            }
            OutboundMessage::Subscribe { .. } | OutboundMessage::Raw(_) => Ok(()),
        }
    }
}
