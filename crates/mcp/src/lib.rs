//! Lightweight MCP (Model Context Protocol) client, exposed as an [`oort_agent::ToolExecutor`].
//!
//! - Transports: [`StdioTransport`] (local subprocess) and [`StreamableHttpTransport`] (remote,
//!   custom headers + pluggable [`TokenProvider`] for bearer/OAuth tokens).
//! - [`McpToolExecutor`]: snapshots `tools/list` (sorted, deterministic schemas) and maps
//!   `annotations.readOnlyHint` to `Tool::is_read_only` (missing ⇒ not read-only).
//! - Errors: `isError: true` and JSON-RPC errors become `is_error` tool results; transport
//!   failures are classified ([`TransportError::kind`]) and logged.
//!
//! Combine several servers (and local tools) with [`oort_agent::CompositeToolExecutor`] or
//! [`McpConfig::connect_into`].

pub mod client;
pub mod config;
pub mod executor;
pub mod oauth;
pub mod protocol;
pub mod record;
pub mod transport;

pub use client::{ClientOptions, McpClient, McpError};
pub use config::{McpConfig, ServerEntry};
pub use executor::{McpToolExecutor, McpToolSpec};
pub use protocol::{
    CallToolResult, Implementation, InitializeResult, McpContent, McpTool, RpcError,
    ToolAnnotations,
};
pub use record::{Probe, connect_record, probe, transport_for};
pub use transport::{
    HttpServerConfig, McpTransport, StaticToken, StdioServerConfig, StdioTransport,
    StreamableHttpTransport, TokenProvider, TransportError,
};

#[cfg(test)]
mod tests;
