//! Connecting to a server described by a stored [`McpServerRecord`].

use std::sync::Arc;
use std::time::Instant;

use oort_agent::secret;
use oort_agent::{McpAuth, McpServerRecord, McpTransportKind};

use crate::client::{ClientOptions, McpClient, McpError};
use crate::protocol::{InitializeResult, McpTool};
use crate::transport::{
    HttpServerConfig, McpTransport, StaticToken, StdioServerConfig, StdioTransport,
    StreamableHttpTransport, TokenProvider,
};

fn config_err(e: impl std::fmt::Display) -> McpError {
    McpError::Config(e.to_string())
}

/// Builds the transport, resolving `env:` references in env values, headers and tokens.
/// `oauth` supplies tokens for servers whose auth is `oauth` (see [`crate::oauth`]).
pub fn transport_for(
    rec: &McpServerRecord,
    oauth: Option<Arc<dyn TokenProvider>>,
) -> Result<Arc<dyn McpTransport>, McpError> {
    Ok(match rec.transport {
        McpTransportKind::Stdio => {
            let command = rec
                .command
                .as_deref()
                .map(str::trim)
                .filter(|c| !c.is_empty())
                .ok_or_else(|| config_err("stdio server needs a command"))?;
            let cfg = StdioServerConfig {
                command: command.to_string(),
                args: rec.args.clone(),
                env: secret::resolve_map(&rec.env).map_err(config_err)?,
                cwd: None,
            };
            Arc::new(StdioTransport::spawn(&rec.name, &cfg)?)
        }
        McpTransportKind::StreamableHttp => {
            let url = rec
                .url
                .as_deref()
                .map(str::trim)
                .filter(|u| !u.is_empty())
                .ok_or_else(|| config_err("streamable_http server needs a url"))?;
            let token = match &rec.auth {
                McpAuth::None => None,
                McpAuth::Bearer { token } => {
                    Some(Arc::new(StaticToken(secret::resolve(token).map_err(config_err)?)) as _)
                }
                McpAuth::OAuth { .. } => Some(oauth.ok_or_else(|| {
                    config_err("this server uses OAuth but no token provider was given")
                })?),
            };
            Arc::new(StreamableHttpTransport::new(HttpServerConfig {
                url: url.to_string(),
                headers: secret::resolve_map(&rec.headers).map_err(config_err)?,
                token,
            })?)
        }
    })
}

/// Connects and completes the handshake.
pub async fn connect_record(
    rec: &McpServerRecord,
    options: ClientOptions,
    oauth: Option<Arc<dyn TokenProvider>>,
) -> Result<McpClient, McpError> {
    let transport = transport_for(rec, oauth)?;
    McpClient::connect(&rec.name, transport, options).await
}

/// Result of a one-off connection test.
#[derive(Debug, Clone)]
pub struct Probe {
    pub latency_ms: u64,
    pub server: InitializeResult,
    pub tools: Vec<McpTool>,
}

/// Connects, runs `initialize` + `tools/list` and disconnects.
pub async fn probe(
    rec: &McpServerRecord,
    options: ClientOptions,
    oauth: Option<Arc<dyn TokenProvider>>,
) -> Result<Probe, McpError> {
    let started = Instant::now();
    let client = connect_record(rec, options, oauth).await?;
    let tools = client.list_tools().await;
    let latency_ms = started.elapsed().as_millis() as u64;
    let server = client.server_info().clone();
    client.close().await;
    let mut tools = tools?;
    tools.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(Probe {
        latency_ms,
        server,
        tools,
    })
}
