//! Tool definitions (schema + metadata) and the execution backend, kept separate.
//!
//! - [`Tool`]: what the model sees (name, description, JSON Schema) plus scheduling metadata.
//! - [`ToolExecutor`]: where calls run (local process, sandbox, MCP server, …).

use std::collections::BTreeMap;
use std::sync::Arc;

use async_trait::async_trait;
use oort_llm::{ToolCall, ToolDefinition};
use serde_json::Value;
use thiserror::Error;
use tokio_util::sync::CancellationToken;

use crate::error::AgentError;

/// Key used to wrap tool arguments that the model emitted as invalid JSON.
/// The call is kept in history as `{ INVALID_ARGUMENTS_KEY: "<raw>" }` (a valid object for
/// every provider) and answered with an error result instead of being executed.
pub const INVALID_ARGUMENTS_KEY: &str = "__invalid_json_arguments__";

/// Static description of a tool.
pub trait Tool: Send + Sync {
    /// Schema sent to the model. Must be deterministic (no `HashMap`-backed values).
    fn definition(&self) -> ToolDefinition;

    /// Read-only tools may run concurrently with each other; others run sequentially.
    fn is_read_only(&self) -> bool {
        false
    }
}

/// Per-call execution context.
#[derive(Debug, Clone)]
pub struct ToolContext {
    pub call_id: String,
    /// Cancelled on `Interrupt` or session cancel. Long-running tools should observe it.
    pub cancel: CancellationToken,
}

/// Successful tool output (which may still be a model-visible error, e.g. non-zero exit).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolOutput {
    pub content: String,
    pub is_error: bool,
}

impl ToolOutput {
    pub fn ok(content: impl Into<String>) -> Self {
        Self {
            content: content.into(),
            is_error: false,
        }
    }

    pub fn error(content: impl Into<String>) -> Self {
        Self {
            content: content.into(),
            is_error: true,
        }
    }
}

/// Tool-level failure. Always converted into `ToolResult { is_error: true }`, never fatal.
#[derive(Debug, Error)]
pub enum ToolError {
    #[error("unknown tool: {0}")]
    NotFound(String),
    #[error("invalid arguments: {0}")]
    InvalidArguments(String),
    #[error("{0}")]
    Failed(String),
    #[error("cancelled")]
    Cancelled,
}

/// Execution backend. Implementations must be cheap to share (`Arc<dyn ToolExecutor>`).
#[async_trait]
pub trait ToolExecutor: Send + Sync {
    /// The tool catalog. Read **once** at agent construction and frozen.
    fn tools(&self) -> Vec<Arc<dyn Tool>>;

    /// Executes a single call.
    async fn execute(&self, call: &ToolCall, ctx: ToolContext) -> Result<ToolOutput, ToolError>;
}

/// Tool catalog frozen at session start: sorted by name, immutable afterwards.
#[derive(Clone)]
pub struct FrozenToolset {
    definitions: Vec<ToolDefinition>,
    read_only: BTreeMap<String, bool>,
}

impl FrozenToolset {
    /// Snapshots `executor.tools()`, sorts by name and rejects duplicates.
    pub fn freeze(executor: &dyn ToolExecutor) -> Result<Self, AgentError> {
        let mut tools = executor.tools();
        tools.sort_by_key(|t| t.definition().name);
        let mut definitions = Vec::with_capacity(tools.len());
        let mut read_only = BTreeMap::new();
        for tool in tools {
            let def = tool.definition();
            if read_only
                .insert(def.name.clone(), tool.is_read_only())
                .is_some()
            {
                return Err(AgentError::InvalidConfig(format!(
                    "duplicate tool name: {}",
                    def.name
                )));
            }
            definitions.push(def);
        }
        Ok(Self {
            definitions,
            read_only,
        })
    }

    pub fn definitions(&self) -> &[ToolDefinition] {
        &self.definitions
    }

    /// Unknown tools are treated as not read-only (sequential).
    pub fn is_read_only(&self, name: &str) -> bool {
        self.read_only.get(name).copied().unwrap_or(false)
    }
}

/// A tool implemented in-process.
#[async_trait]
pub trait LocalTool: Tool {
    async fn run(&self, arguments: Value, ctx: ToolContext) -> Result<ToolOutput, ToolError>;
}

/// [`ToolExecutor`] that dispatches to in-process [`LocalTool`]s by name.
#[derive(Default)]
pub struct LocalToolExecutor {
    tools: BTreeMap<String, Arc<dyn LocalTool>>,
}

impl LocalToolExecutor {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_tool(mut self, tool: Arc<dyn LocalTool>) -> Self {
        self.tools.insert(tool.definition().name, tool);
        self
    }
}

#[async_trait]
impl ToolExecutor for LocalToolExecutor {
    fn tools(&self) -> Vec<Arc<dyn Tool>> {
        self.tools
            .values()
            .map(|t| Arc::clone(t) as Arc<dyn Tool>)
            .collect()
    }

    async fn execute(&self, call: &ToolCall, ctx: ToolContext) -> Result<ToolOutput, ToolError> {
        let tool = self
            .tools
            .get(&call.name)
            .ok_or_else(|| ToolError::NotFound(call.name.clone()))?;
        tool.run(call.arguments.clone(), ctx).await
    }
}
