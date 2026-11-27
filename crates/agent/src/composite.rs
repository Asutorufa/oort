//! Routing several [`ToolExecutor`]s (MCP servers, local tools, …) behind one catalog.

use std::collections::BTreeMap;
use std::sync::Arc;

use async_trait::async_trait;
use oort_llm::{ToolCall, ToolDefinition};

use crate::error::AgentError;
use crate::tool::{Tool, ToolContext, ToolError, ToolExecutor, ToolOutput};

/// Max tool-name length accepted by every major provider (`^[a-zA-Z0-9_-]{1,64}$`).
pub const MAX_TOOL_NAME_LEN: usize = 64;

/// How a source's tool names are exposed to the model.
///
/// Prefixing is explicit (never "only on conflict"): adding a server must not rename the tools
/// of another, or previously stored sessions would no longer match their frozen catalog.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Namespace {
    /// Expose names as-is.
    None,
    /// Expose as `{prefix}__{name}`.
    Prefix(String),
}

struct Route {
    executor: Arc<dyn ToolExecutor>,
    original_name: String,
    tool: Arc<dyn Tool>,
}

/// A [`Tool`] exposed under a different name.
struct RenamedTool {
    name: String,
    inner: Arc<dyn Tool>,
}

impl Tool for RenamedTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: self.name.clone(),
            ..self.inner.definition()
        }
    }

    fn is_read_only(&self) -> bool {
        self.inner.is_read_only()
    }
}

/// Routes calls by (exposed) tool name to the executor that owns the tool.
pub struct CompositeToolExecutor {
    routes: BTreeMap<String, Route>,
}

/// Builder for [`CompositeToolExecutor`].
#[derive(Default)]
pub struct CompositeBuilder {
    sources: Vec<(Namespace, Arc<dyn ToolExecutor>)>,
}

impl CompositeBuilder {
    pub fn add(mut self, namespace: Namespace, executor: Arc<dyn ToolExecutor>) -> Self {
        self.sources.push((namespace, executor));
        self
    }

    /// Fails on name collisions (after namespacing and sanitizing) instead of renaming silently.
    pub fn build(self) -> Result<CompositeToolExecutor, AgentError> {
        let mut routes = BTreeMap::new();
        for (ns, executor) in self.sources {
            for tool in executor.tools() {
                let original_name = tool.definition().name;
                let exposed = exposed_name(&ns, &original_name);
                if routes.contains_key(&exposed) {
                    return Err(AgentError::InvalidConfig(format!(
                        "tool name collision: `{exposed}` (use a Namespace::Prefix)"
                    )));
                }
                let tool: Arc<dyn Tool> = if exposed == original_name {
                    tool
                } else {
                    Arc::new(RenamedTool {
                        name: exposed.clone(),
                        inner: tool,
                    })
                };
                routes.insert(
                    exposed,
                    Route {
                        executor: Arc::clone(&executor),
                        original_name,
                        tool,
                    },
                );
            }
        }
        Ok(CompositeToolExecutor { routes })
    }
}

impl CompositeToolExecutor {
    pub fn builder() -> CompositeBuilder {
        CompositeBuilder::default()
    }
}

#[async_trait]
impl ToolExecutor for CompositeToolExecutor {
    fn tools(&self) -> Vec<Arc<dyn Tool>> {
        self.routes.values().map(|r| Arc::clone(&r.tool)).collect()
    }

    async fn execute(&self, call: &ToolCall, ctx: ToolContext) -> Result<ToolOutput, ToolError> {
        let route = self
            .routes
            .get(&call.name)
            .ok_or_else(|| ToolError::NotFound(call.name.clone()))?;
        let mut inner = call.clone();
        inner.name = route.original_name.clone();
        route.executor.execute(&inner, ctx).await
    }
}

/// `{prefix}__{name}`, with characters outside `[a-zA-Z0-9_-]` replaced by `_`, truncated to 64.
pub fn exposed_name(ns: &Namespace, name: &str) -> String {
    let raw = match ns {
        Namespace::None => name.to_string(),
        Namespace::Prefix(p) => format!("{p}__{name}"),
    };
    raw.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .take(MAX_TOOL_NAME_LEN)
        .collect()
}

/// A frozen catalog (e.g. a stored session's `frozen_tools`) served by whatever executor is
/// live now.
///
/// The model always sees exactly `pinned`, so a resumed session's tool prefix stays
/// byte-identical however the live servers changed. Live tools that are not pinned are hidden;
/// a pinned tool the live executor no longer offers answers with a tool error (an `is_error`
/// result for the model), never an agent failure. Scheduling (`is_read_only`) follows the live
/// tool when present and is conservative (sequential) otherwise.
pub struct PinnedToolExecutor {
    live: Arc<dyn ToolExecutor>,
    tools: Vec<Arc<dyn Tool>>,
    available: BTreeMap<String, bool>,
}

struct PinnedTool {
    definition: ToolDefinition,
    read_only: bool,
}

impl Tool for PinnedTool {
    fn definition(&self) -> ToolDefinition {
        self.definition.clone()
    }
    fn is_read_only(&self) -> bool {
        self.read_only
    }
}

impl PinnedToolExecutor {
    pub fn new(pinned: &[ToolDefinition], live: Arc<dyn ToolExecutor>) -> Self {
        let live_tools: BTreeMap<String, bool> = live
            .tools()
            .iter()
            .map(|t| (t.definition().name, t.is_read_only()))
            .collect();
        let tools = pinned
            .iter()
            .map(|d| {
                Arc::new(PinnedTool {
                    definition: d.clone(),
                    read_only: live_tools.get(&d.name).copied().unwrap_or(false),
                }) as Arc<dyn Tool>
            })
            .collect();
        let available = pinned
            .iter()
            .filter_map(|d| live_tools.get(&d.name).map(|ro| (d.name.clone(), *ro)))
            .collect();
        Self {
            live,
            tools,
            available,
        }
    }

    /// Pinned tool names the live executor does not offer any more.
    pub fn missing(&self) -> Vec<String> {
        self.tools
            .iter()
            .map(|t| t.definition().name)
            .filter(|n| !self.available.contains_key(n))
            .collect()
    }
}

#[async_trait]
impl ToolExecutor for PinnedToolExecutor {
    fn tools(&self) -> Vec<Arc<dyn Tool>> {
        self.tools.clone()
    }

    async fn execute(&self, call: &ToolCall, ctx: ToolContext) -> Result<ToolOutput, ToolError> {
        if !self.available.contains_key(&call.name) {
            return Err(ToolError::Failed(format!(
                "tool `{}` is no longer available (its server was changed or removed after this session started)",
                call.name
            )));
        }
        self.live.execute(call, ctx).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{Behavior, MockTool, executor};
    use tokio_util::sync::CancellationToken;

    fn ctx() -> ToolContext {
        ToolContext {
            call_id: "1".into(),
            cancel: CancellationToken::new(),
        }
    }

    #[tokio::test]
    async fn routes_by_namespaced_name_and_restores_original() {
        let local = executor(&[MockTool::new("read", true, Behavior::Ok("local".into()))]);
        let gh = executor(&[
            MockTool::new("read", true, Behavior::Ok("github".into())),
            MockTool::new("create.issue", false, Behavior::Ok("issue".into())),
        ]);
        let c = CompositeToolExecutor::builder()
            .add(Namespace::None, local)
            .add(Namespace::Prefix("github".into()), gh)
            .build()
            .unwrap();
        let names: Vec<String> = c.tools().iter().map(|t| t.definition().name).collect();
        assert_eq!(names, ["github__create_issue", "github__read", "read"]);
        assert!(c.tools()[1].is_read_only());

        let call = |n: &str| ToolCall::new("1", n, serde_json::json!({}));
        assert!(
            c.execute(&call("read"), ctx())
                .await
                .unwrap()
                .content
                .starts_with("local")
        );
        assert!(
            c.execute(&call("github__read"), ctx())
                .await
                .unwrap()
                .content
                .starts_with("github")
        );
        assert!(
            c.execute(&call("github__create_issue"), ctx())
                .await
                .unwrap()
                .content
                .starts_with("issue")
        );
        assert!(matches!(
            c.execute(&call("nope"), ctx()).await,
            Err(ToolError::NotFound(_))
        ));
    }

    #[test]
    fn collisions_are_rejected_and_names_are_sanitized() {
        let a = executor(&[MockTool::new("read", true, Behavior::Ok("".into()))]);
        let b = executor(&[MockTool::new("read", true, Behavior::Ok("".into()))]);
        assert!(
            CompositeToolExecutor::builder()
                .add(Namespace::None, a)
                .add(Namespace::None, b)
                .build()
                .is_err()
        );
        let long = "x".repeat(100);
        assert_eq!(
            exposed_name(&Namespace::Prefix("my server".into()), "a/b"),
            "my_server__a_b"
        );
        assert_eq!(
            exposed_name(&Namespace::None, &long).len(),
            MAX_TOOL_NAME_LEN
        );
    }
}
