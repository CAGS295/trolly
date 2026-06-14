//! Signed REST helpers for USDM user-data `listenKey` lifecycle.

use std::collections::BTreeMap;
use std::fmt;

use reqwest::blocking::Client as BlockingClient;
use reqwest::Client as AsyncClient;
use serde::Deserialize;

use crate::auth::{sign_params, signed_params_payload};
use crate::endpoints::ApiCredentials;

pub const LISTEN_KEY_PATH: &str = "/fapi/v1/listenKey";

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct ListenKeyResponse {
    #[serde(rename = "listenKey")]
    pub listen_key: String,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
struct BinanceApiError {
    code: i64,
    msg: String,
}

#[derive(Debug)]
pub enum ListenKeyError {
    Http(reqwest::Error),
    Api { code: i64, msg: String },
    InvalidResponse(String),
}

impl fmt::Display for ListenKeyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Http(err) => write!(f, "http error: {err}"),
            Self::Api { code, msg } => write!(f, "binance api error {code}: {msg}"),
            Self::InvalidResponse(msg) => write!(f, "invalid listen key response: {msg}"),
        }
    }
}

impl std::error::Error for ListenKeyError {}

impl From<reqwest::Error> for ListenKeyError {
    fn from(value: reqwest::Error) -> Self {
        Self::Http(value)
    }
}

/// REST client for `listenKey` create/keepalive on a configured futures REST base URL.
#[derive(Clone, Debug)]
pub struct ListenKeyClient {
    credentials: ApiCredentials,
    rest_base: String,
    client: AsyncClient,
}

impl ListenKeyClient {
    pub fn new(credentials: ApiCredentials, rest_base: impl Into<String>) -> Self {
        Self {
            credentials,
            rest_base: rest_base.into().trim_end_matches('/').to_string(),
            client: AsyncClient::new(),
        }
    }

    fn listen_key_url(&self) -> String {
        format!("{}{LISTEN_KEY_PATH}", self.rest_base)
    }

    fn signed_empty_body(&self) -> Result<String, ListenKeyError> {
        let params = sign_params(&self.credentials.secret_key, BTreeMap::new());
        Ok(signed_params_payload(&params))
    }

    pub async fn create(&self) -> Result<ListenKeyResponse, ListenKeyError> {
        let query = self.signed_empty_body()?;
        let response = self
            .client
            .post(self.listen_key_url())
            .header("X-MBX-APIKEY", &self.credentials.api_key)
            .header("Content-Type", "application/x-www-form-urlencoded")
            .body(query)
            .send()
            .await?;

        Self::parse_response(response).await
    }

    pub fn create_blocking(&self) -> Result<ListenKeyResponse, ListenKeyError> {
        let query = self.signed_empty_body()?;
        let response = BlockingClient::new()
            .post(self.listen_key_url())
            .header("X-MBX-APIKEY", &self.credentials.api_key)
            .header("Content-Type", "application/x-www-form-urlencoded")
            .body(query)
            .send()?;

        Self::parse_response_blocking(response)
    }

    pub async fn keepalive(&self) -> Result<(), ListenKeyError> {
        let query = self.signed_empty_body()?;
        let response = self
            .client
            .put(self.listen_key_url())
            .header("X-MBX-APIKEY", &self.credentials.api_key)
            .header("Content-Type", "application/x-www-form-urlencoded")
            .body(query)
            .send()
            .await?;

        Self::parse_keepalive_response(response).await
    }

    pub fn keepalive_blocking(&self) -> Result<(), ListenKeyError> {
        let query = self.signed_empty_body()?;
        let response = BlockingClient::new()
            .put(self.listen_key_url())
            .header("X-MBX-APIKEY", &self.credentials.api_key)
            .header("Content-Type", "application/x-www-form-urlencoded")
            .body(query)
            .send()?;

        Self::parse_keepalive_response_blocking(response)
    }

    async fn parse_response(
        response: reqwest::Response,
    ) -> Result<ListenKeyResponse, ListenKeyError> {
        let status = response.status();
        let body = response.text().await?;
        Self::decode_body(status, &body)
    }

    fn parse_response_blocking(
        response: reqwest::blocking::Response,
    ) -> Result<ListenKeyResponse, ListenKeyError> {
        let status = response.status();
        let body = response.text()?;
        Self::decode_body(status, &body)
    }

    fn decode_body(
        status: reqwest::StatusCode,
        body: &str,
    ) -> Result<ListenKeyResponse, ListenKeyError> {
        if let Ok(api_err) = serde_json::from_str::<BinanceApiError>(body) {
            return Err(ListenKeyError::Api {
                code: api_err.code,
                msg: api_err.msg,
            });
        }

        if !status.is_success() {
            return Err(ListenKeyError::InvalidResponse(format!(
                "status {status}: {body}"
            )));
        }

        serde_json::from_str(body).map_err(|err| ListenKeyError::InvalidResponse(err.to_string()))
    }

    async fn parse_keepalive_response(response: reqwest::Response) -> Result<(), ListenKeyError> {
        let status = response.status();
        let body = response.text().await?;
        Self::decode_keepalive_body(status, &body)
    }

    fn parse_keepalive_response_blocking(
        response: reqwest::blocking::Response,
    ) -> Result<(), ListenKeyError> {
        let status = response.status();
        let body = response.text()?;
        Self::decode_keepalive_body(status, &body)
    }

    fn decode_keepalive_body(
        status: reqwest::StatusCode,
        body: &str,
    ) -> Result<(), ListenKeyError> {
        if let Ok(api_err) = serde_json::from_str::<BinanceApiError>(body) {
            return Err(ListenKeyError::Api {
                code: api_err.code,
                msg: api_err.msg,
            });
        }

        if !status.is_success() {
            return Err(ListenKeyError::InvalidResponse(format!(
                "status {status}: {body}"
            )));
        }

        Ok(())
    }
}
