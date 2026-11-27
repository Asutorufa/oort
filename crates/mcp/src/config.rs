//! `{"mcpServers": {...}}` configuration (the format used by common MCP hosts).

use std::collections::BTreeMap;
use std::sync::Arc;

use oort_agent::{CompositeBuilder, Namespace, ToolExecutor};
use serde::Deserialize;

use crate::client::{ClientOptions, McpClient, McpError};
use crate::executor::McpToolExecutor;
use crate::transport::{
    HttpServerConfig, McpTransport, StaticToken, StdioServerConfig, StdioTransport,
    StreamableHttpTransport,
};

/// One server entry: either a subprocess (`command`) or a remote endpoint (`url`).
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum ServerEntry {
    Stdio(StdioServerConfig),
    Http {
        url: String,
        #[serde(default)]
        headers: BTreeMap<String, String>,
        /// Convenience for a static `Authorization: Bearer` token.
        #[serde(default, rename = "bearerToken")]
        bearer_token: Option<String>,
    },
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct McpConfig {
    #[serde(rename = "mcpServers", default)]
    pub servers: BTreeMap<String, ServerEntry>,
}

impl McpConfig {
    pub fn from_json(raw: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(raw)
    }

    /// Connects every server (in name order) and adds each to `builder` under `Namespace::Prefix(name)`.
    pub async fn connect_into(
        &self,
        mut builder: CompositeBuilder,
        options: &ClientOptions,
    ) -> Result<(CompositeBuilder, Vec<Arc<McpClient>>), McpError> {
        let mut clients = Vec::new();
        for (name, entry) in &self.servers {
            let transport: Arc<dyn McpTransport> = match entry {
                ServerEntry::Stdio(cfg) => Arc::new(StdioTransport::spawn(name, cfg)?),
                ServerEntry::Http {
                    url,
                    headers,
                    bearer_token,
                } => Arc::new(StreamableHttpTransport::new(HttpServerConfig {
                    url: url.clone(),
                    headers: headers.clone(),
                    token: bearer_token.clone().map(|t| Arc::new(StaticToken(t)) as _),
                })?),
            };
            let client = Arc::new(McpClient::connect(name, transport, options.clone()).await?);
            let executor: Arc<dyn ToolExecutor> =
                Arc::new(McpToolExecutor::new(client.clone()).await?);
            builder = builder.add(Namespace::Prefix(name.clone()), executor);
            clients.push(client);
        }
        Ok((builder, clients))
    }
}
