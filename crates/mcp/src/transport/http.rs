//! Streamable HTTP transport (MCP 2025-03-26+): every message is a POST to one endpoint; the
//! server answers with `application/json` or a `text/event-stream` carrying the response.

use std::collections::BTreeMap;
use std::sync::Arc;

use async_trait::async_trait;
use futures_util::StreamExt;
use reqwest::header::{ACCEPT, AUTHORIZATION, CONTENT_TYPE, HeaderMap, HeaderName, HeaderValue};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::Mutex;

use super::{McpTransport, TransportError};
use crate::protocol::{self, Incoming, RpcError};

const SESSION_HEADER: &str = "mcp-session-id";
const VERSION_HEADER: &str = "mcp-protocol-version";

/// Supplies bearer tokens. Implement this to plug in OAuth (refresh in `token`, drop the cached
/// token in `invalidate`); a 401 triggers one `invalidate` + retry.
#[async_trait]
pub trait TokenProvider: Send + Sync {
    async fn token(&self) -> Result<String, TransportError>;
    async fn invalidate(&self) {}
}

/// A fixed bearer token.
pub struct StaticToken(pub String);

#[async_trait]
impl TokenProvider for StaticToken {
    async fn token(&self) -> Result<String, TransportError> {
        Ok(self.0.clone())
    }
}

/// Remote server endpoint.
#[derive(Clone, Default, Serialize, Deserialize)]
pub struct HttpServerConfig {
    pub url: String,
    /// Extra headers sent with every request (sorted for deterministic behavior).
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
    #[serde(skip)]
    pub token: Option<Arc<dyn TokenProvider>>,
}

impl std::fmt::Debug for HttpServerConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HttpServerConfig")
            .field("url", &self.url)
            .field("headers", &self.headers.keys().collect::<Vec<_>>())
            .field("token", &self.token.is_some())
            .finish()
    }
}

pub struct StreamableHttpTransport {
    http: reqwest::Client,
    cfg: HttpServerConfig,
    headers: HeaderMap,
    session_id: Mutex<Option<String>>,
    protocol_version: Mutex<Option<String>>,
}

impl StreamableHttpTransport {
    pub fn new(cfg: HttpServerConfig) -> Result<Self, TransportError> {
        let mut headers = HeaderMap::new();
        for (k, v) in &cfg.headers {
            let name = HeaderName::try_from(k.as_str())
                .map_err(|e| TransportError::Protocol(format!("header {k}: {e}")))?;
            let value = HeaderValue::try_from(v.as_str())
                .map_err(|e| TransportError::Protocol(format!("header {k}: {e}")))?;
            headers.insert(name, value);
        }
        Ok(Self {
            http: reqwest::Client::new(),
            cfg,
            headers,
            session_id: Mutex::new(None),
            protocol_version: Mutex::new(None),
        })
    }

    async fn post(
        &self,
        body: &Value,
        retry_auth: bool,
    ) -> Result<reqwest::Response, TransportError> {
        let mut req = self
            .http
            .post(&self.cfg.url)
            .headers(self.headers.clone())
            .header(CONTENT_TYPE, "application/json")
            .header(ACCEPT, "application/json, text/event-stream")
            .json(body);
        if let Some(sid) = self.session_id.lock().await.clone() {
            req = req.header(SESSION_HEADER, sid);
        }
        if let Some(v) = self.protocol_version.lock().await.clone() {
            req = req.header(VERSION_HEADER, v);
        }
        if let Some(tp) = &self.cfg.token {
            req = req.header(AUTHORIZATION, format!("Bearer {}", tp.token().await?));
        }
        let resp = req.send().await.map_err(map_reqwest)?;
        match resp.status().as_u16() {
            401 if retry_auth && self.cfg.token.is_some() => {
                if let Some(tp) = &self.cfg.token {
                    tp.invalidate().await;
                }
                Box::pin(self.post(body, false)).await
            }
            401 => Err(TransportError::Unauthorized),
            404 if self.session_id.lock().await.is_some() => {
                Err(TransportError::Disconnected("MCP session expired".into()))
            }
            s if !(200..300).contains(&s) => Err(TransportError::Http {
                status: s,
                body: resp.text().await.unwrap_or_default(),
            }),
            _ => {
                if let Some(sid) = resp
                    .headers()
                    .get(SESSION_HEADER)
                    .and_then(|v| v.to_str().ok())
                {
                    *self.session_id.lock().await = Some(sid.to_string());
                }
                Ok(resp)
            }
        }
    }

    /// Finds the response to `id` in a JSON or SSE body, answering server requests on the way.
    async fn read_response(
        &self,
        resp: reqwest::Response,
        id: u64,
    ) -> Result<Result<Value, RpcError>, TransportError> {
        let is_sse = resp
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .is_some_and(|ct| ct.starts_with("text/event-stream"));
        if !is_sse {
            let body: Value = resp
                .json()
                .await
                .map_err(|e| TransportError::Protocol(e.to_string()))?;
            let messages = match body {
                Value::Array(items) => items,
                other => vec![other],
            };
            for m in &messages {
                if let Some(r) = self.handle_incoming(m, id).await {
                    return Ok(r);
                }
            }
            return Err(TransportError::Protocol(format!(
                "no response for request {id}"
            )));
        }

        let mut events = oort_llm::sse::parse_sse_stream(Box::pin(resp.bytes_stream()));
        while let Some(ev) = events.next().await {
            let ev = ev.map_err(|e| TransportError::Disconnected(e.to_string()))?;
            if ev.data.trim().is_empty() {
                continue;
            }
            let Ok(m) = serde_json::from_str::<Value>(&ev.data) else {
                tracing::warn!(url = %self.cfg.url, "ignoring non-JSON SSE event");
                continue;
            };
            if let Some(r) = self.handle_incoming(&m, id).await {
                return Ok(r);
            }
        }
        Err(TransportError::Disconnected(format!(
            "stream ended before response {id}"
        )))
    }

    async fn handle_incoming(&self, m: &Value, want: u64) -> Option<Result<Value, RpcError>> {
        match Incoming::parse(m)? {
            Incoming::Response { id, result } if id == want => Some(result),
            Incoming::Response { id, .. } => {
                tracing::warn!(id, "unexpected response id");
                None
            }
            Incoming::Request { id, method } => {
                let reply = protocol::reply_to_server_request(&id, &method);
                if let Err(e) = self.post(&reply, true).await {
                    tracing::warn!(error = %e, "failed to answer server request");
                }
                None
            }
            Incoming::Notification { method } => {
                tracing::debug!(%method, "server notification");
                None
            }
        }
    }
}

fn map_reqwest(e: reqwest::Error) -> TransportError {
    if e.is_timeout() {
        TransportError::Timeout
    } else if e.is_connect() {
        TransportError::Disconnected(e.to_string())
    } else {
        TransportError::Io(e.to_string())
    }
}

#[async_trait]
impl McpTransport for StreamableHttpTransport {
    async fn request(
        &self,
        id: u64,
        method: &str,
        params: Option<Value>,
    ) -> Result<Result<Value, RpcError>, TransportError> {
        let resp = self
            .post(&protocol::request(id, method, params), true)
            .await?;
        let result = self.read_response(resp, id).await?;
        if method == "initialize"
            && let Ok(v) = &result
            && let Some(ver) = v.get("protocolVersion").and_then(Value::as_str)
        {
            *self.protocol_version.lock().await = Some(ver.to_string());
        }
        Ok(result)
    }

    async fn notify(&self, method: &str, params: Option<Value>) -> Result<(), TransportError> {
        self.post(&protocol::notification(method, params), true)
            .await
            .map(drop)
    }

    /// Ends the server-side session (best effort).
    async fn close(&self) {
        let Some(sid) = self.session_id.lock().await.take() else {
            return;
        };
        let _ = self
            .http
            .delete(&self.cfg.url)
            .headers(self.headers.clone())
            .header(SESSION_HEADER, sid)
            .send()
            .await;
    }
}
