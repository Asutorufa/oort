//! Transports: how JSON-RPC messages reach an MCP server.

mod http;
mod stdio;

pub use http::{HttpServerConfig, StaticToken, StreamableHttpTransport, TokenProvider};
pub use stdio::{StdioServerConfig, StdioTransport};

use async_trait::async_trait;
use serde_json::Value;
use thiserror::Error;

use crate::protocol::RpcError;

/// Transport-level failures (as opposed to tool or JSON-RPC errors).
#[derive(Debug, Clone, Error)]
pub enum TransportError {
    #[error("request timed out")]
    Timeout,
    /// The connection is gone (process exited, stream closed, session expired).
    #[error("disconnected: {0}")]
    Disconnected(String),
    #[error("unauthorized")]
    Unauthorized,
    #[error("HTTP {status}: {body}")]
    Http { status: u16, body: String },
    /// The peer sent something that is not valid MCP / JSON-RPC.
    #[error("protocol error: {0}")]
    Protocol(String),
    #[error("I/O error: {0}")]
    Io(String),
}

impl TransportError {
    /// Short stable label for logs and metrics.
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Timeout => "timeout",
            Self::Disconnected(_) => "disconnected",
            Self::Unauthorized => "unauthorized",
            Self::Http { .. } => "http",
            Self::Protocol(_) => "protocol",
            Self::Io(_) => "io",
        }
    }
}

/// A bidirectional JSON-RPC channel to one MCP server.
///
/// Request ids are allocated by the caller ([`crate::McpClient`]) so it can reference them in
/// `notifications/cancelled`. Dropping a pending `request` future must release its resources.
#[async_trait]
pub trait McpTransport: Send + Sync {
    /// Sends request `id` and waits for its response.
    async fn request(
        &self,
        id: u64,
        method: &str,
        params: Option<Value>,
    ) -> Result<Result<Value, RpcError>, TransportError>;

    /// Sends a notification (no response).
    async fn notify(&self, method: &str, params: Option<Value>) -> Result<(), TransportError>;

    /// Gracefully shuts the connection down (terminates subprocesses, ends HTTP sessions).
    async fn close(&self);
}
