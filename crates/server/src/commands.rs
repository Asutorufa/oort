//! `/` commands: deterministic, zero-token controls, listed together with invocable skills.

use oort_agent::{ApprovalPolicyKind, SkillExecutionMode};
use serde::{Deserialize, Serialize};

use crate::api::{ApiError, CreateSessionRequest, InputRequest, PatchSessionRequest};
use crate::manager::SessionManager;

#[derive(Debug, Clone, Serialize)]
pub struct CommandInfo {
    pub name: String,
    /// `command` (built in) or `skill`.
    pub kind: &'static str,
    pub description: String,
    pub argument_hint: Option<String>,
    /// Skills in `subagent` mode run in an isolated subagent.
    pub subagent: bool,
    /// Commands that only make sense inside a session.
    pub needs_session: bool,
}

const BUILTINS: [(&str, &str, Option<&str>, bool); 5] = [
    (
        "compact",
        "Summarize the conversation so far to free up context",
        None,
        true,
    ),
    (
        "approval",
        "Switch the approval policy",
        Some("<auto|readonly|ask>"),
        true,
    ),
    ("stop", "Stop the agent now", None, true),
    ("export", "Download this conversation as JSON", None, true),
    (
        "new",
        "Start a new chat (optionally from a preset)",
        Some("[preset]"),
        false,
    ),
];

pub async fn list(m: &SessionManager) -> Result<Vec<CommandInfo>, ApiError> {
    let mut out: Vec<CommandInfo> = BUILTINS
        .iter()
        .map(|(n, d, h, s)| CommandInfo {
            name: n.to_string(),
            kind: "command",
            description: d.to_string(),
            argument_hint: h.map(str::to_string),
            subagent: false,
            needs_session: *s,
        })
        .collect();
    for k in m.registry().skills.list().await? {
        if !k.invocation.user_invocable || BUILTINS.iter().any(|(n, ..)| *n == k.name) {
            continue;
        }
        out.push(CommandInfo {
            subagent: k.invocation.execution_mode == SkillExecutionMode::Subagent,
            argument_hint: k.invocation.argument_hint.clone(),
            description: k.description.clone(),
            name: k.name,
            kind: "skill",
            needs_session: false,
        });
    }
    Ok(out)
}

#[derive(Debug, Clone, Deserialize)]
pub struct RunCommand {
    #[serde(default)]
    pub session_id: Option<String>,
    pub name: String,
    #[serde(default)]
    pub arguments: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct CommandEffect {
    /// Open this session.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub navigate_session: Option<String>,
    /// Download this URL.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub download: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct CommandResult {
    pub message: String,
    pub effect: CommandEffect,
}

pub async fn run(m: &SessionManager, req: RunCommand) -> Result<CommandResult, ApiError> {
    let args = req.arguments.as_deref().map(str::trim).unwrap_or("");
    let session = || {
        req.session_id.clone().ok_or_else(|| {
            ApiError::BadRequest(format!("/{} needs an open conversation", req.name))
        })
    };
    let done = |message: &str| CommandResult {
        message: message.to_string(),
        effect: CommandEffect::default(),
    };
    Ok(match req.name.as_str() {
        "compact" => {
            m.input(&session()?, InputRequest::Compact).await?;
            done("Compaction requested")
        }
        "approval" => {
            let policy = match args {
                "auto" | "auto_all" | "all" => ApprovalPolicyKind::AutoAll,
                "readonly" | "auto_readonly" | "read-only" => ApprovalPolicyKind::AutoReadonly,
                "ask" | "always_ask" => ApprovalPolicyKind::AlwaysAsk,
                other => {
                    return Err(ApiError::BadRequest(format!(
                        "/approval takes auto, readonly or ask (got `{other}`)"
                    )));
                }
            };
            m.patch(
                &session()?,
                PatchSessionRequest {
                    title: None,
                    approval_policy: Some(policy),
                    reasoning_effort: None,
                },
            )
            .await?;
            done(match policy {
                ApprovalPolicyKind::AutoAll => "Approval: run every tool automatically",
                ApprovalPolicyKind::AutoReadonly => "Approval: read-only tools run automatically",
                ApprovalPolicyKind::AlwaysAsk => "Approval: ask every time",
            })
        }
        "stop" => {
            let stopped = m.cancel(&session()?).await?;
            done(if stopped {
                "Stopped"
            } else {
                "Nothing was running"
            })
        }
        "export" => {
            let id = session()?;
            m.detail(&id).await?;
            CommandResult {
                message: "Export ready".into(),
                effect: CommandEffect {
                    download: Some(format!("/api/sessions/{id}/export")),
                    ..Default::default()
                },
            }
        }
        "new" => {
            let mut create = CreateSessionRequest::default();
            if !args.is_empty() {
                let preset = m
                    .registry()
                    .config
                    .list_presets()
                    .await?
                    .into_iter()
                    .find(|p| p.name.eq_ignore_ascii_case(args) || p.id == args)
                    .ok_or_else(|| ApiError::BadRequest(format!("unknown preset `{args}`")))?;
                create.preset_id = Some(preset.id);
            } else if let Some(id) = &req.session_id {
                // Same model and tools as the current conversation.
                let d = m.detail(id).await?;
                create.provider_id = d.settings.provider_id.clone();
                create.model = Some(d.settings.model.clone());
                create.mcp_server_ids = Some(
                    d.settings
                        .mcp_servers
                        .iter()
                        .map(|b| b.id.clone())
                        .collect(),
                );
                create.builtin_groups = Some(d.meta.lineage.builtin_groups.clone());
                create.workspace_root = d.meta.lineage.workspace_root.clone();
                create.approval_policy = Some(d.meta.approval_policy);
            } else {
                create.provider_id = Some("mock".into());
            }
            let id = m.create(create).await?;
            CommandResult {
                message: "New chat started".into(),
                effect: CommandEffect {
                    navigate_session: Some(id),
                    ..Default::default()
                },
            }
        }
        other => return Err(ApiError::NotFound(format!("command /{other}"))),
    })
}
