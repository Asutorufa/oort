//! Group registry, shared context and approval previews.

pub mod core_ux;
pub mod html;
pub mod memory;
pub mod self_config;
pub mod utilities;
pub mod workspace;

use std::path::PathBuf;
use std::sync::Arc;

use oort_agent::{ConfigStore, LocalToolExecutor, MemoryStore, PlanStep, SkillStore, ToolError};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Tool groups a session can enable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Group {
    SelfConfig,
    CoreUx,
    Memory,
    Utilities,
    /// `spawn_subagent` (provided by the application, which owns sessions and models).
    Subagent,
    /// Local files and commands under `workspace_root` (off unless a root is given).
    Workspace,
}

impl Group {
    pub const ALL: [Group; 6] = [
        Group::SelfConfig,
        Group::CoreUx,
        Group::Memory,
        Group::Utilities,
        Group::Subagent,
        Group::Workspace,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Group::SelfConfig => "self_config",
            Group::CoreUx => "core_ux",
            Group::Memory => "memory",
            Group::Utilities => "utilities",
            Group::Subagent => "subagent",
            Group::Workspace => "workspace",
        }
    }

    pub fn parse(s: &str) -> Option<Group> {
        Group::ALL.into_iter().find(|g| g.as_str() == s)
    }
}

/// Tools that need a human decision even when the approval policy approves everything.
pub const ALWAYS_ASK: [&str; 3] = ["configure_mcp_server", "run_command", "save_skill"];

/// The tool whose result is the user's answer.
pub const ASK_USER: &str = "ask_user";

/// Receives plan updates (the application turns them into `AgentEvent::PlanUpdated`).
pub trait PlanSink: Send + Sync {
    fn publish(&self, steps: Vec<PlanStep>);
}

/// What the builtin tools may touch. Unused handles may be `None` when their group is off.
#[derive(Clone)]
pub struct BuiltinContext {
    pub config: Option<Arc<dyn ConfigStore>>,
    pub skills: Option<Arc<dyn SkillStore>>,
    pub memories: Option<Arc<dyn MemoryStore>>,
    pub plan: Option<Arc<dyn PlanSink>>,
    /// Canonical workspace root (required by the `workspace` group).
    pub workspace_root: Option<PathBuf>,
    pub web: utilities::WebFetchOptions,
    pub commands: workspace::CommandOptions,
}

impl Default for BuiltinContext {
    fn default() -> Self {
        Self {
            config: None,
            skills: None,
            memories: None,
            plan: None,
            workspace_root: None,
            web: utilities::WebFetchOptions::default(),
            commands: workspace::CommandOptions::default(),
        }
    }
}

fn missing(what: &str) -> ToolError {
    ToolError::Failed(format!("{what} is not available in this session"))
}

/// The local tools of the enabled groups (`subagent` is the application's).
pub fn executor(groups: &[Group], ctx: &BuiltinContext) -> Result<LocalToolExecutor, String> {
    let mut ex = LocalToolExecutor::new();
    for g in groups {
        ex = match g {
            Group::SelfConfig => self_config::add(ex, ctx),
            Group::CoreUx => core_ux::add(ex, ctx),
            Group::Memory => memory::add(ex, ctx),
            Group::Utilities => utilities::add(ex, ctx),
            Group::Workspace => {
                if ctx.workspace_root.is_none() {
                    return Err("the workspace group needs a workspace_root".into());
                }
                workspace::add(ex, ctx)
            }
            Group::Subagent => ex,
        };
    }
    Ok(ex)
}

/// What an approval card shows for a pending call (diff, command, settings), or `None`.
pub async fn preview(tool: &str, args: &Value, ctx: &BuiltinContext) -> Option<Value> {
    match tool {
        "write_file" => workspace::write_preview(args, ctx).await,
        "run_command" => workspace::command_preview(args, ctx),
        "save_skill" => self_config::skill_preview(args, ctx).await,
        "configure_mcp_server" => Some(self_config::mcp_preview(args)),
        _ => None,
    }
}

pub(crate) fn str_arg<'a>(args: &'a Value, key: &str) -> Result<&'a str, ToolError> {
    args.get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| ToolError::InvalidArguments(format!("missing string argument `{key}`")))
}

pub(crate) fn opt_str<'a>(args: &'a Value, key: &str) -> Option<&'a str> {
    args.get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.trim().is_empty())
}

pub(crate) fn str_list(args: &Value, key: &str) -> Vec<String> {
    args.get(key)
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

pub(crate) fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or_default()
}

/// Head and tail of long output, with a marker in between.
pub(crate) fn trim_middle(s: &str, max_chars: usize) -> String {
    let n = s.chars().count();
    if n <= max_chars {
        return s.to_string();
    }
    let head: String = s.chars().take(max_chars * 2 / 3).collect();
    let tail: String = s.chars().skip(n - max_chars / 3).collect();
    format!(
        "{head}\n…[{} characters omitted]…\n{tail}",
        n - head.chars().count() - tail.chars().count()
    )
}

/// Declares a tool's definition from JSON in one place.
macro_rules! tool_def {
    ($ty:ident, $name:expr, $read_only:expr, $desc:expr, $schema:tt) => {
        impl oort_agent::Tool for $ty {
            fn definition(&self) -> oort_llm::ToolDefinition {
                oort_llm::ToolDefinition::new($name, $desc, serde_json::json!($schema))
            }
            fn is_read_only(&self) -> bool {
                $read_only
            }
        }
    };
}
pub(crate) use tool_def;
