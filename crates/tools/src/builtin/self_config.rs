//! `save_skill` and `configure_mcp_server`: the agent edits oort's own configuration.
//! Both always ask the user ([`super::ALWAYS_ASK`]); configuration changes apply to *new*
//! sessions only, never to the running one's frozen tool catalog.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use oort_agent::secret;
use oort_agent::{
    ConfigStore, LocalTool, LocalToolExecutor, McpAuth, McpServerRecord, McpTransportKind,
    SkillExecutionMode, SkillInvocation, SkillRecord, SkillStore, ToolContext, ToolError,
    ToolOutput,
};
use serde_json::{Value, json};

use super::{BuiltinContext, missing, now_ms, opt_str, str_arg, str_list, tool_def};

fn valid_skill_name(name: &str) -> bool {
    (1..=64).contains(&name.len())
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        && !name.starts_with('-')
        && !name.ends_with('-')
}

fn store_err(e: oort_agent::StoreError) -> ToolError {
    ToolError::Failed(e.to_string())
}

// ----- save_skill ----------------------------------------------------------------------------

pub struct SaveSkill {
    store: Option<Arc<dyn SkillStore>>,
}

tool_def!(SaveSkill, "save_skill", false,
"Save a procedure established in this conversation as a reusable skill (or update the app skill with this name). The user reviews it before it is saved.",
{
    "type": "object",
    "properties": {
        "name": { "type": "string", "description": "lowercase letters, digits and -; also the /command" },
        "description": { "type": "string", "description": "when to use it, in one sentence" },
        "body": { "type": "string", "description": "Markdown instructions; may use $ARGUMENTS" },
        "tags": { "type": "array", "items": { "type": "string" } },
        "allowed_tools": { "type": "array", "items": { "type": "string" } },
        "argument_hint": { "type": "string" },
        "execution_mode": { "type": "string", "enum": ["inline", "subagent"] }
    },
    "required": ["name", "description", "body"]
});

fn skill_from_args(args: &Value) -> Result<SkillRecord, ToolError> {
    let name = str_arg(args, "name")?.trim().to_string();
    if !valid_skill_name(&name) {
        return Err(ToolError::InvalidArguments(
            "name must be 1-64 characters of [a-z0-9-]".into(),
        ));
    }
    let now = now_ms();
    Ok(SkillRecord {
        id: uuid::Uuid::now_v7().to_string(),
        name,
        description: str_arg(args, "description")?.trim().to_string(),
        body: str_arg(args, "body")?.to_string(),
        allowed_tools: str_list(args, "allowed_tools"),
        tags: str_list(args, "tags"),
        invocation: SkillInvocation {
            user_invocable: true,
            argument_hint: opt_str(args, "argument_hint").map(str::to_string),
            execution_mode: if opt_str(args, "execution_mode") == Some("subagent") {
                SkillExecutionMode::Subagent
            } else {
                SkillExecutionMode::Inline
            },
            subagent_profile: None,
        },
        created_at: now,
        updated_at: now,
    })
}

#[async_trait]
impl LocalTool for SaveSkill {
    async fn run(&self, args: Value, _ctx: ToolContext) -> Result<ToolOutput, ToolError> {
        let store = self
            .store
            .as_ref()
            .ok_or_else(|| missing("skill storage"))?;
        let mut rec = skill_from_args(&args)?;
        let existing = store
            .list_skills()
            .await
            .map_err(store_err)?
            .into_iter()
            .find(|s| s.name == rec.name);
        let verb = if let Some(cur) = existing {
            rec.id = cur.id;
            rec.created_at = cur.created_at;
            rec.invocation.subagent_profile = cur.invocation.subagent_profile;
            store.update_skill(&rec).await.map_err(store_err)?;
            "Updated"
        } else {
            store.create_skill(&rec).await.map_err(store_err)?;
            "Saved"
        };
        Ok(ToolOutput::ok(format!(
            "{verb} skill /{} — available as a /command now and to new sessions' skill lists.",
            rec.name
        )))
    }
}

pub async fn skill_preview(args: &Value, ctx: &BuiltinContext) -> Option<Value> {
    let rec = skill_from_args(args).ok()?;
    let exists = match &ctx.skills {
        Some(s) => s
            .list_skills()
            .await
            .ok()?
            .iter()
            .any(|k| k.name == rec.name),
        None => false,
    };
    Some(json!({
        "kind": "skill",
        "update": exists,
        "name": rec.name,
        "description": rec.description,
        "body": rec.body,
        "tags": rec.tags,
        "argument_hint": rec.invocation.argument_hint,
        "execution_mode": rec.invocation.execution_mode,
    }))
}

// ----- configure_mcp_server ------------------------------------------------------------------

pub struct ConfigureMcpServer {
    store: Option<Arc<dyn ConfigStore>>,
}

tool_def!(ConfigureMcpServer, "configure_mcp_server", false,
"Register or update an MCP server (by name) and optionally test it. It becomes available to NEW sessions only; this session's tools do not change. Always asks the user first.",
{
    "type": "object",
    "properties": {
        "name": { "type": "string", "description": "Server identifier (1-32 chars of [A-Za-z0-9_-], no `__`); becomes the tool prefix" },
        "transport": { "type": "string", "enum": ["streamable_http", "stdio"], "description": "`streamable_http` (requires `url`) or `stdio` (requires `command`)" },
        "url": { "type": "string", "description": "HTTP(S) endpoint URL (required when transport is `streamable_http`)" },
        "headers": { "type": "object", "additionalProperties": { "type": "string" }, "description": "Optional HTTP headers map {header_name: value} (streamable_http only)" },
        "command": { "type": "string", "description": "Executable to spawn (required when transport is `stdio`)" },
        "args": { "type": "array", "items": { "type": "string" }, "description": "Optional CLI arguments for `command` (stdio only)" },
        "env": { "type": "object", "additionalProperties": { "type": "string" }, "description": "Optional environment variables map {VAR: value} (stdio only)" },
        "test": { "type": "boolean", "description": "Connect and list tools after saving (default true)" }
    },
    "required": ["name", "transport"]
});

fn map_arg(args: &Value, key: &str) -> BTreeMap<String, String> {
    args.get(key)
        .and_then(Value::as_object)
        .map(|o| {
            o.iter()
                .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
                .collect()
        })
        .unwrap_or_default()
}

fn server_from_args(args: &Value) -> Result<McpServerRecord, ToolError> {
    let name = str_arg(args, "name")?.trim().to_string();
    let name_ok = (1..=32).contains(&name.len())
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        && !name.contains("__");
    if !name_ok {
        return Err(ToolError::InvalidArguments(
            "name must be 1-32 characters of [A-Za-z0-9_-] without `__`".into(),
        ));
    }
    let transport = match str_arg(args, "transport")? {
        "streamable_http" => McpTransportKind::StreamableHttp,
        "stdio" => McpTransportKind::Stdio,
        other => {
            return Err(ToolError::InvalidArguments(format!(
                "unknown transport {other}"
            )));
        }
    };
    let url = opt_str(args, "url").map(str::to_string);
    let command = opt_str(args, "command").map(str::to_string);
    match transport {
        McpTransportKind::StreamableHttp if url.is_none() => {
            return Err(ToolError::InvalidArguments(
                "streamable_http needs a url".into(),
            ));
        }
        McpTransportKind::Stdio if command.is_none() => {
            return Err(ToolError::InvalidArguments("stdio needs a command".into()));
        }
        _ => {}
    }
    let now = now_ms();
    Ok(McpServerRecord {
        id: uuid::Uuid::now_v7().to_string(),
        name,
        transport,
        url,
        command,
        args: str_list(args, "args"),
        env: map_arg(args, "env"),
        headers: map_arg(args, "headers"),
        auth: McpAuth::None,
        is_enabled: true,
        created_at: now,
        updated_at: now,
    })
}

#[async_trait]
impl LocalTool for ConfigureMcpServer {
    async fn run(&self, args: Value, ctx: ToolContext) -> Result<ToolOutput, ToolError> {
        let store = self
            .store
            .as_ref()
            .ok_or_else(|| missing("configuration storage"))?;
        let mut rec = server_from_args(&args)?;
        let existing = store
            .list_mcp_servers()
            .await
            .map_err(store_err)?
            .into_iter()
            .find(|s| s.name == rec.name);
        let created = existing.is_none();
        if let Some(cur) = existing {
            rec.id = cur.id;
            rec.created_at = cur.created_at;
            rec.auth = cur.auth;
            rec.updated_at = now_ms().max(cur.updated_at + 1);
            store.update_mcp_server(&rec).await.map_err(store_err)?;
        } else {
            store.create_mcp_server(&rec).await.map_err(store_err)?;
        }

        let test = if args.get("test").and_then(Value::as_bool).unwrap_or(true) {
            let opts = oort_mcp::ClientOptions {
                request_timeout: Duration::from_secs(15),
                ..Default::default()
            };
            let probe = tokio::select! {
                _ = ctx.cancel.cancelled() => return Err(ToolError::Cancelled),
                p = oort_mcp::probe(&rec, opts, None) => p,
            };
            Some(match probe {
                Ok(p) => {
                    json!({ "ok": true, "server": p.server.server_info.name, "tools": p.tools.iter().map(|t| t.name.clone()).collect::<Vec<_>>() })
                }
                Err(e) => json!({ "ok": false, "error": e.to_string() }),
            })
        } else {
            None
        };
        let discovered = test
            .as_ref()
            .and_then(|t| t.get("tools").cloned())
            .unwrap_or(json!([]));
        Ok(ToolOutput::ok(
            json!({
                "action": "suggest_new_session",
                "mcp_server_id": rec.id,
                "name": rec.name,
                "created": created,
                "test": test,
                "discovered_tools": discovered,
                "note": "Saved. This session's tools are unchanged; start a new session to use this server.",
            })
            .to_string(),
        ))
    }
}

/// Approval preview with secrets masked.
pub fn mcp_preview(args: &Value) -> Value {
    match server_from_args(args) {
        Ok(r) => json!({
            "kind": "mcp_server",
            "name": r.name,
            "transport": r.transport,
            "url": r.url,
            "command": r.command,
            "args": r.args,
            "env": secret::mask_map(&r.env),
            "headers": secret::mask_map(&r.headers),
        }),
        Err(e) => json!({ "kind": "error", "error": e.to_string() }),
    }
}

pub(super) fn add(ex: LocalToolExecutor, ctx: &BuiltinContext) -> LocalToolExecutor {
    ex.with_tool(Arc::new(SaveSkill {
        store: ctx.skills.clone(),
    }))
    .with_tool(Arc::new(ConfigureMcpServer {
        store: ctx.config.clone(),
    }))
}
