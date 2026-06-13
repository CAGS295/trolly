//! REST `listenKey` lifecycle for USDM user-data streams (caller-owned; demo or production base).

use serde::Deserialize;
use thiserror::Error;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

use crate::endpoints::{ApiCredentials, USDM_DEMO_REST_BASE_URL, USDM_REST_BASE_URL};

/// Errors from listen-key REST calls.
#[derive(Debug, Error)]
pub enum ListenKeyError {
    #[error("http transport: {0}")]
    Transport(String),
    #[error("unexpected response status {status}: {body}")]
    UnexpectedStatus { status: u16, body: String },
    #[error("failed to parse response: {0}")]
    Parse(String),
}

/// Manages `POST` / `PUT` / `DELETE /fapi/v1/listenKey` against a configurable REST base.
#[derive(Clone, Debug)]
pub struct ListenKeyClient {
    pub rest_base_url: String,
    pub credentials: ApiCredentials,
}

impl ListenKeyClient {
    pub fn production(credentials: ApiCredentials) -> Self {
        Self::with_rest_base(USDM_REST_BASE_URL, credentials)
    }

    pub fn demo(credentials: ApiCredentials) -> Self {
        Self::with_rest_base(USDM_DEMO_REST_BASE_URL, credentials)
    }

    pub fn with_rest_base(rest_base_url: impl Into<String>, credentials: ApiCredentials) -> Self {
        Self {
            rest_base_url: rest_base_url.into().trim_end_matches('/').to_string(),
            credentials,
        }
    }

    fn listen_key_url(&self) -> String {
        format!("{}/fapi/v1/listenKey", self.rest_base_url)
    }

    /// `POST /fapi/v1/listenKey` — create a new user-data stream key.
    pub async fn create(&self) -> Result<String, ListenKeyError> {
        let response = request_native_tls(
            "POST",
            &self.listen_key_url(),
            &self.credentials.api_key,
            None,
        )
        .await?;
        if response.status != 200 {
            return Err(ListenKeyError::UnexpectedStatus {
                status: response.status,
                body: response.body,
            });
        }
        let parsed: ListenKeyResponse = serde_json::from_str(&response.body)
            .map_err(|e| ListenKeyError::Parse(e.to_string()))?;
        Ok(parsed.listen_key)
    }

    /// `PUT /fapi/v1/listenKey` — extend the key validity (~60 minutes).
    pub async fn keepalive(&self) -> Result<(), ListenKeyError> {
        let response = request_native_tls(
            "PUT",
            &self.listen_key_url(),
            &self.credentials.api_key,
            None,
        )
        .await?;
        if response.status != 200 {
            return Err(ListenKeyError::UnexpectedStatus {
                status: response.status,
                body: response.body,
            });
        }
        Ok(())
    }

    /// `DELETE /fapi/v1/listenKey` — close the user-data stream.
    pub async fn close(&self) -> Result<(), ListenKeyError> {
        let response = request_native_tls(
            "DELETE",
            &self.listen_key_url(),
            &self.credentials.api_key,
            None,
        )
        .await?;
        if response.status != 200 {
            return Err(ListenKeyError::UnexpectedStatus {
                status: response.status,
                body: response.body,
            });
        }
        Ok(())
    }
}

#[derive(Debug, Deserialize)]
struct ListenKeyResponse {
    #[serde(rename = "listenKey")]
    listen_key: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct HttpResponse {
    status: u16,
    body: String,
}

async fn request_native_tls(
    method: &str,
    url: &str,
    api_key: &str,
    body: Option<&str>,
) -> Result<HttpResponse, ListenKeyError> {
    let (host, port, path) = parse_https_url(url)?;
    let connector = native_tls::TlsConnector::builder()
        .build()
        .map_err(|e| ListenKeyError::Transport(e.to_string()))?;
    let connector = tokio_native_tls::TlsConnector::from(connector);

    let tcp = TcpStream::connect((host.as_str(), port))
        .await
        .map_err(|e| ListenKeyError::Transport(e.to_string()))?;
    let mut stream = connector
        .connect(host.as_str(), tcp)
        .await
        .map_err(|e| ListenKeyError::Transport(e.to_string()))?;

    let body = body.unwrap_or("");
    let request = format!(
        "{method} {path} HTTP/1.1\r\n\
         Host: {host}\r\n\
         X-MBX-APIKEY: {api_key}\r\n\
         Content-Length: {len}\r\n\
         Connection: close\r\n\
         \r\n\
         {body}",
        method = method,
        path = path,
        host = host,
        api_key = api_key,
        len = body.len(),
        body = body,
    );
    stream
        .write_all(request.as_bytes())
        .await
        .map_err(|e| ListenKeyError::Transport(e.to_string()))?;
    stream
        .shutdown()
        .await
        .map_err(|e| ListenKeyError::Transport(e.to_string()))?;

    let mut raw = Vec::new();
    stream
        .read_to_end(&mut raw)
        .await
        .map_err(|e| ListenKeyError::Transport(e.to_string()))?;

    parse_http_response(&raw)
}

fn parse_https_url(url: &str) -> Result<(String, u16, String), ListenKeyError> {
    let rest = url
        .strip_prefix("https://")
        .ok_or_else(|| ListenKeyError::Transport("only https URLs are supported".into()))?;
    let (authority, path) = match rest.split_once('/') {
        Some((authority, path)) => (authority, format!("/{path}")),
        None => (rest, "/".into()),
    };
    let (host, port) = match authority.split_once(':') {
        Some((host, port)) => (
            host.to_string(),
            port.parse()
                .map_err(|_| ListenKeyError::Transport("invalid port".into()))?,
        ),
        None => (authority.to_string(), 443),
    };
    Ok((host, port, path))
}

fn parse_http_response(raw: &[u8]) -> Result<HttpResponse, ListenKeyError> {
    let text = std::str::from_utf8(raw).map_err(|e| ListenKeyError::Transport(e.to_string()))?;
    let (header_block, body) = text
        .split_once("\r\n\r\n")
        .ok_or_else(|| ListenKeyError::Transport("malformed http response".into()))?;
    let status_line = header_block
        .lines()
        .next()
        .ok_or_else(|| ListenKeyError::Transport("missing status line".into()))?;
    let status = status_line
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| ListenKeyError::Transport("invalid status line".into()))?;
    Ok(HttpResponse {
        status,
        body: body.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn demo_client_targets_demo_rest_base() {
        let client = ListenKeyClient::demo(ApiCredentials {
            api_key: "k".into(),
            secret_key: "s".into(),
        });
        assert_eq!(
            client.listen_key_url(),
            "https://demo-fapi.binance.com/fapi/v1/listenKey"
        );
    }
}
