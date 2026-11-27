//! MCP session: handshake, tool listing, tool calls with timeout and cancellation.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use serde_json::{Value, json};
use thiserror::Error;
use tokio_util::sync::CancellationToken;

use crate::protocol::{
    CallToolResult, InitializeResult, ListToolsResult, McpTool, PROTOCOL_VERSION, RpcError,
};
use crate::transport::{McpTransport, TransportError};

#[derive(Debug, Clone, Error)]
pub enum McpError {
    #[error(transparent)]
    Transport(#[from] TransportError),
    #[error(transparent)]
    Rpc(#[from] RpcError),
    #[error("cancelled")]
    Cancelled,
    #[error("invalid response: {0}")]
    Decode(String),
    /// The server configuration cannot be used (missing field, unresolved secret, …).
    #[error("invalid configuration: {0}")]
    Config(String),
}

/// Client options.
#[derive(Debug, Clone)]
pub struct ClientOptions {
    /// Per-request timeout (handshake and `tools/list` included).
    pub request_timeout: Duration,
    pub client_name: String,
}

impl Default for ClientOptions {
    fn default() -> Self {
        Self {
            request_timeout: Duration::from_secs(60),
            client_name: "oort".into(),
        }
    }
}

/// An initialized MCP session over some [`McpTransport`].
pub struct McpClient {
    name: String,
    transport: Arc<dyn McpTransport>,
    next_id: AtomicU64,
    options: ClientOptions,
    server: InitializeResult,
}

impl McpClient {
    /// Runs the `initialize` / `notifications/initialized` handshake.
    pub async fn connect(
        name: impl Into<String>,
        transport: Arc<dyn McpTransport>,
        options: ClientOptions,
    ) -> Result<Self, McpError> {
        let mut client = Self {
            name: name.into(),
            transport,
            next_id: AtomicU64::new(1),
            options,
            server: InitializeResult {
                protocol_version: String::new(),
                capabilities: Value::Null,
                server_info: crate::protocol::Implementation {
                    name: String::new(),
                    version: String::new(),
                },
                instructions: None,
            },
        };
        let params = json!({
            "protocolVersion": PROTOCOL_VERSION,
            "capabilities": {},
            "clientInfo": { "name": client.options.client_name, "version": env!("CARGO_PKG_VERSION") },
        });
        let result = client
            .call("initialize", Some(params), &CancellationToken::new())
            .await?;
        client.server = decode(result)?;
        client
            .transport
            .notify("notifications/initialized", None)
            .await?;
        tracing::info!(server = %client.name, info = ?client.server.server_info, version = %client.server.protocol_version, "MCP connected");
        Ok(client)
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn server_info(&self) -> &InitializeResult {
        &self.server
    }

    /// All tools, following `nextCursor` pagination.
    pub async fn list_tools(&self) -> Result<Vec<McpTool>, McpError> {
        let mut tools = Vec::new();
        let mut cursor: Option<String> = None;
        loop {
            let params = cursor.as_ref().map(|c| json!({ "cursor": c }));
            let page: ListToolsResult = decode(
                self.call("tools/list", params, &CancellationToken::new())
                    .await?,
            )?;
            tools.extend(page.tools);
            match page.next_cursor {
                Some(next) if Some(&next) != cursor.as_ref() => cursor = Some(next),
                _ => return Ok(tools),
            }
        }
    }

    pub async fn call_tool(
        &self,
        name: &str,
        arguments: Value,
        cancel: &CancellationToken,
    ) -> Result<CallToolResult, McpError> {
        decode(self.call_tool_raw(name, arguments, cancel).await?)
    }

    /// `tools/call` returning the result object verbatim (binary content included).
    pub async fn call_tool_raw(
        &self,
        name: &str,
        arguments: Value,
        cancel: &CancellationToken,
    ) -> Result<Value, McpError> {
        let params = json!({ "name": name, "arguments": arguments });
        self.call("tools/call", Some(params), cancel).await
    }

    pub async fn close(&self) {
        self.transport.close().await;
    }

    /// One request with timeout and cooperative cancellation (`notifications/cancelled`).
    async fn call(
        &self,
        method: &str,
        params: Option<Value>,
        cancel: &CancellationToken,
    ) -> Result<Value, McpError> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let fut = tokio::time::timeout(
            self.options.request_timeout,
            self.transport.request(id, method, params),
        );
        let outcome = tokio::select! {
            biased;
            _ = cancel.cancelled() => {
                self.send_cancelled(id, "cancelled by client").await;
                return Err(McpError::Cancelled);
            }
            r = fut => r,
        };
        match outcome {
            Err(_elapsed) => {
                self.send_cancelled(id, "timeout").await;
                Err(TransportError::Timeout.into())
            }
            Ok(Err(e)) => Err(e.into()),
            Ok(Ok(Err(rpc))) => Err(rpc.into()),
            Ok(Ok(Ok(v))) => Ok(v),
        }
    }

    async fn send_cancelled(&self, id: u64, reason: &str) {
        let params = json!({ "requestId": id, "reason": reason });
        if let Err(e) = self
            .transport
            .notify("notifications/cancelled", Some(params))
            .await
        {
            tracing::debug!(server = %self.name, error = %e, "could not send cancellation");
        }
    }
}

fn decode<T: serde::de::DeserializeOwned>(v: Value) -> Result<T, McpError> {
    serde_json::from_value(v).map_err(|e| McpError::Decode(e.to_string()))
}
