//! USDM user-data `listenKey` lifecycle on a REST host (demo or production).

use reqwest::header::{HeaderMap, HeaderValue};
use serde::Deserialize;
use thiserror::Error;

use crate::endpoints::ApiCredentials;

#[derive(Debug, Error)]
pub enum ListenKeyError {
    #[error("http error: {0}")]
    Http(#[from] reqwest::Error),
    #[error("api error {status}: {body}")]
    Api { status: u16, body: String },
    #[error("missing listenKey in response")]
    MissingListenKey,
}

#[derive(Debug, Deserialize)]
struct ListenKeyResponse {
    #[serde(rename = "listenKey")]
    listen_key: String,
}

/// Signed REST client for `POST` / `PUT` / `DELETE` `/fapi/v1/listenKey`.
#[derive(Clone, Debug)]
pub struct ListenKeyClient {
    base_url: String,
    credentials: ApiCredentials,
    http: reqwest::Client,
}

impl ListenKeyClient {
    pub fn new(base_url: impl Into<String>, credentials: ApiCredentials) -> Self {
        Self {
            base_url: base_url.into(),
            credentials,
            http: reqwest::Client::new(),
        }
    }

    fn endpoint(&self) -> String {
        format!("{}/fapi/v1/listenKey", self.base_url.trim_end_matches('/'))
    }

    fn api_key_headers(&self) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(
            "X-MBX-APIKEY",
            HeaderValue::from_str(&self.credentials.api_key).expect("valid api key header"),
        );
        headers
    }

    pub async fn create(&self) -> Result<String, ListenKeyError> {
        let response = self
            .http
            .post(self.endpoint())
            .headers(self.api_key_headers())
            .send()
            .await?;
        let status = response.status();
        let body = response.text().await?;
        if !status.is_success() {
            return Err(ListenKeyError::Api {
                status: status.as_u16(),
                body,
            });
        }
        let parsed: ListenKeyResponse = serde_json::from_str(&body).map_err(|_| {
            ListenKeyError::Api {
                status: status.as_u16(),
                body,
            }
        })?;
        Ok(parsed.listen_key)
    }

    pub async fn keepalive(&self) -> Result<String, ListenKeyError> {
        let response = self
            .http
            .put(self.endpoint())
            .headers(self.api_key_headers())
            .send()
            .await?;
        let status = response.status();
        let body = response.text().await?;
        if !status.is_success() {
            return Err(ListenKeyError::Api {
                status: status.as_u16(),
                body,
            });
        }
        let parsed: ListenKeyResponse = serde_json::from_str(&body).map_err(|_| {
            ListenKeyError::Api {
                status: status.as_u16(),
                body,
            }
        })?;
        Ok(parsed.listen_key)
    }

    pub async fn close(&self) -> Result<(), ListenKeyError> {
        let response = self
            .http
            .delete(self.endpoint())
            .headers(self.api_key_headers())
            .send()
            .await?;
        let status = response.status();
        if status.is_success() {
            return Ok(());
        }
        let body = response.text().await?;
        Err(ListenKeyError::Api {
            status: status.as_u16(),
            body,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn test_credentials() -> ApiCredentials {
        ApiCredentials {
            api_key: "demo-key".into(),
            secret_key: "demo-secret".into(),
        }
    }

    #[tokio::test]
    async fn create_listen_key_posts_with_api_key_header() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/fapi/v1/listenKey"))
            .and(header("X-MBX-APIKEY", "demo-key"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_string(r#"{"listenKey":"test-listen-key"}"#),
            )
            .mount(&server)
            .await;

        let client = ListenKeyClient::new(server.uri(), test_credentials());
        let key = client.create().await.expect("create listen key");
        assert_eq!(key, "test-listen-key");
    }

    #[tokio::test]
    async fn keepalive_and_close_hit_listen_key_endpoint() {
        let server = MockServer::start().await;
        Mock::given(method("PUT"))
            .and(path("/fapi/v1/listenKey"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_string(r#"{"listenKey":"test-listen-key"}"#),
            )
            .mount(&server)
            .await;
        Mock::given(method("DELETE"))
            .and(path("/fapi/v1/listenKey"))
            .respond_with(ResponseTemplate::new(200))
            .mount(&server)
            .await;

        let client = ListenKeyClient::new(server.uri(), test_credentials());
        assert_eq!(
            client.keepalive().await.expect("keepalive"),
            "test-listen-key"
        );
        client.close().await.expect("close listen key");
    }
}
