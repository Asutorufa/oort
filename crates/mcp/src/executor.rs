//! [`ToolExecutor`] backed by one MCP server.

use std::collections::BTreeSet;
use std::sync::Arc;

use async_trait::async_trait;
use oort_agent::{Tool, ToolContext, ToolError, ToolExecutor, ToolOutput};
use oort_llm::{ToolCall, ToolDefinition};
use serde_json::{Value, json};

use crate::client::{McpClient, McpError};
use crate::protocol::{McpTool, RpcError};

/// Tool metadata from `tools/list`.
pub struct McpToolSpec {
    definition: ToolDefinition,
    read_only: bool,
}

impl Tool for McpToolSpec {
    fn definition(&self) -> ToolDefinition {
        self.definition.clone()
    }

    fn is_read_only(&self) -> bool {
        self.read_only
    }
}

impl McpToolSpec {
    pub fn from_mcp(tool: &McpTool) -> Self {
        Self {
            definition: ToolDefinition::new(
                &tool.name,
                tool.description
                    .clone()
                    .or_else(|| tool.title.clone())
                    .unwrap_or_default(),
                normalize_schema(&tool.input_schema),
            ),
            read_only: tool.is_read_only(),
        }
    }
}

/// An object schema is required by every provider; a missing/invalid one becomes `{"type":"object"}`.
fn normalize_schema(schema: &Value) -> Value {
    match schema {
        Value::Object(map) if !map.is_empty() => schema.clone(),
        _ => json!({ "type": "object" }),
    }
}

/// Exposes the tools of one MCP server. The catalog is snapshotted once at connect time,
/// sorted by name, and never changes afterwards (`notifications/tools/list_changed` is ignored
/// on purpose: the agent's frozen catalog — and the prefix cache — must stay stable).
pub struct McpToolExecutor {
    client: Arc<McpClient>,
    tools: Vec<Arc<McpToolSpec>>,
}

impl McpToolExecutor {
    pub async fn new(client: Arc<McpClient>) -> Result<Self, McpError> {
        let mut listed = client.list_tools().await?;
        listed.sort_by(|a, b| a.name.cmp(&b.name));
        let mut seen = BTreeSet::new();
        let mut tools = Vec::with_capacity(listed.len());
        for t in &listed {
            if !seen.insert(t.name.clone()) {
                return Err(McpError::Decode(format!(
                    "server listed tool `{}` twice",
                    t.name
                )));
            }
            tools.push(Arc::new(McpToolSpec::from_mcp(t)));
        }
        Ok(Self { client, tools })
    }

    pub fn client(&self) -> &Arc<McpClient> {
        &self.client
    }
}

#[async_trait]
impl ToolExecutor for McpToolExecutor {
    fn tools(&self) -> Vec<Arc<dyn Tool>> {
        self.tools
            .iter()
            .map(|t| Arc::clone(t) as Arc<dyn Tool>)
            .collect()
    }

    async fn execute(&self, call: &ToolCall, ctx: ToolContext) -> Result<ToolOutput, ToolError> {
        let server = self.client.name();
        match self
            .client
            .call_tool(&call.name, call.arguments.clone(), &ctx.cancel)
            .await
        {
            Ok(result) => {
                let text = result.to_text();
                Ok(if result.is_error {
                    ToolOutput::error(text)
                } else {
                    ToolOutput::ok(text)
                })
            }
            Err(McpError::Cancelled) => Err(ToolError::Cancelled),
            Err(McpError::Rpc(e)) if e.code == RpcError::INVALID_PARAMS => {
                Err(ToolError::InvalidArguments(e.message))
            }
            Err(McpError::Rpc(e)) => Err(ToolError::Failed(format!(
                "MCP server error {}: {}",
                e.code, e.message
            ))),
            Err(McpError::Transport(e)) => {
                tracing::error!(%server, tool = %call.name, kind = e.kind(), error = %e, "MCP transport failure");
                Err(ToolError::Failed(format!(
                    "MCP transport error ({}): {e}",
                    e.kind()
                )))
            }
            Err(McpError::Decode(e)) => {
                tracing::error!(%server, tool = %call.name, error = %e, "invalid MCP tool result");
                Err(ToolError::Failed(format!("invalid MCP tool result: {e}")))
            }
            Err(e @ McpError::Config(_)) => Err(ToolError::Failed(e.to_string())),
        }
    }
}
