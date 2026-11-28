//! Subagents: an isolated child session run as one tool call of its parent.
//!
//! - `spawn_subagent` is a regular read-only tool, so several run concurrently (`JoinSet`).
//! - The child's `session_cancel` is a child of the parent call's token: an interrupt or stop of
//!   the parent cancels the child and its tools; the parent then records the usual synthetic
//!   "interrupted" result.
//! - Least privilege: the child gets read-only tools only and a guard hook that rejects writes,
//!   questions to the user and spawning beyond `max_depth`.
//! - Budget: after `max_turns` the child is told to summarize without tools; a timeout returns
//!   what it said so far. Its trial and error stays in its own session (stored with
//!   `parent_session_id`); the parent only receives the final summary.
//! - `fresh` starts from a profile prompt; `fork` copies the parent's history prefix and tool
//!   catalog byte for byte, so the provider can reuse the parent's cached prefix.

use std::collections::BTreeSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Weak};
use std::time::Duration;

use async_trait::async_trait;
use oort_agent::{
    Agent, AgentEvent, AgentState, ApprovalPolicyKind, BeforeLlm, ContentBlock, FrozenToolset,
    HookAction, Inbox, Lineage, LocalTool, LoopHook, NewSession, PersistenceHook,
    PinnedToolExecutor, Tool, ToolContext, ToolDecision, ToolError, ToolExecutor, ToolOutput,
    TracingHook,
};
use oort_llm::{Message, Role, ToolCall, ToolDefinition};
use serde_json::{Value, json};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::api::ApiError;
use crate::config::SessionConfig;
use crate::registry::{Registry, ToolSelection, parse_groups};

pub const SPAWN_SUBAGENT: &str = "spawn_subagent";

pub const DENY_WRITE: &str =
    "Subagent is read-only. Return your proposed changes to the parent agent for user approval.";

/// Global subagent limits.
#[derive(Debug, Clone)]
pub struct SubagentLimits {
    /// Sessions at this depth cannot spawn (1 = only top-level sessions spawn).
    pub max_depth: u32,
    pub default_max_turns: u32,
    pub timeout: Duration,
}

impl Default for SubagentLimits {
    fn default() -> Self {
        Self {
            max_depth: 1,
            default_max_turns: 12,
            timeout: Duration::from_secs(300),
        }
    }
}

/// What a running session gives its `spawn_subagent` tool.
#[derive(Clone)]
pub struct SubagentRuntime {
    pub session_id: String,
    pub cfg: SessionConfig,
    pub lineage: Lineage,
    pub store: Arc<dyn oort_agent::SessionStore>,
    /// The parent's event channel (child events are relayed as `AgentEvent::Subagent`).
    pub events: mpsc::Sender<AgentEvent>,
}

/// A subagent profile: who the child is.
#[derive(Debug, Clone)]
pub struct Profile {
    pub name: String,
    pub prompt: String,
    pub max_turns: u32,
    /// `(provider_id, model)` when the profile picks its own model (presets).
    pub model: Option<(String, String)>,
}

const PROFILE_BASE: &str = "You are a subagent working for another agent, not talking to the user. \
Work only on the task you are given, using your tools (all read-only). You cannot ask the user \
questions and cannot change anything; if changes are needed, describe them precisely. When you \
are done, reply with a concise report for the parent agent: what you found, evidence \
(files, URLs, quotes), and open questions.";

pub fn builtin_profiles() -> Vec<(&'static str, &'static str)> {
    vec![
        (
            "explorer",
            "Explore and map: locate the relevant files, symbols, settings or sources quickly; prefer breadth, then report where things are and how they connect.",
        ),
        (
            "researcher",
            "Research: gather facts from the web and saved memories, compare sources, and report a sourced answer with uncertainties called out.",
        ),
        (
            "reviewer",
            "Review critically: look for bugs, risks and inconsistencies in what you are pointed at; list issues by severity with evidence and concrete fixes.",
        ),
    ]
}

impl Registry {
    /// Resolves `explorer` / `researcher` / `reviewer` or `preset:<name>`.
    pub async fn subagent_profile(&self, name: &str) -> Result<Profile, ApiError> {
        if let Some(preset) = name.strip_prefix("preset:") {
            let p = self
                .config
                .list_presets()
                .await?
                .into_iter()
                .find(|p| p.name == preset || p.id == preset)
                .ok_or_else(|| ApiError::BadRequest(format!("unknown preset `{preset}`")))?;
            return Ok(Profile {
                name: name.to_string(),
                prompt: format!("{PROFILE_BASE}\n\n{}", p.system_prompt),
                max_turns: self.subagents.default_max_turns,
                model: Some((p.provider_id, p.model)),
            });
        }
        let (n, role) = builtin_profiles()
            .into_iter()
            .find(|(n, _)| *n == name)
            .ok_or_else(|| {
                ApiError::BadRequest(format!(
                    "unknown subagent profile `{name}` (use explorer, researcher, reviewer or preset:<name>)"
                ))
            })?;
        Ok(Profile {
            name: n.to_string(),
            prompt: format!("{PROFILE_BASE}\n\n{role}"),
            max_turns: self.subagents.default_max_turns + if n == "researcher" { 3 } else { 0 },
            model: None,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContextMode {
    Fresh,
    Fork,
}

#[derive(Debug, Clone)]
pub struct SubagentTask {
    pub task: String,
    pub context: Option<String>,
    pub profile: String,
    pub mode: ContextMode,
    /// Narrow the child's tools to these names (still read-only).
    pub tools: Option<Vec<String>>,
}

/// Where child events go (the parent's event channel, or a hub for skill-run subagents).
pub type EventFn = Arc<dyn Fn(AgentEvent) + Send + Sync>;

pub struct SubagentOutcome {
    pub sub_session_id: String,
    pub summary: String,
    /// `completed`, `turn_limit`, `timeout`.
    pub ending: &'static str,
}

/// Guard rails of every child: read-only, no questions, bounded depth.
struct SubagentGuard {
    can_spawn: bool,
}

#[async_trait]
impl LoopHook for SubagentGuard {
    async fn before_tool(&self, _s: &AgentState, call: &ToolCall, read_only: bool) -> ToolDecision {
        if call.name == SPAWN_SUBAGENT && !self.can_spawn {
            return ToolDecision::Deny {
                reason: "maximum subagent depth reached: do the work yourself".into(),
            };
        }
        if call.name == oort_tools::ASK_USER {
            return ToolDecision::Deny {
                reason: "subagents cannot ask the user; put the question in your final report"
                    .into(),
            };
        }
        if !read_only {
            return ToolDecision::Deny {
                reason: DENY_WRITE.into(),
            };
        }
        ToolDecision::Approve
    }
}

/// After `max_turns` LLM calls: one more turn to summarize, tools refused; then halt.
struct TurnBudget {
    max_turns: u32,
    exhausted: AtomicBool,
}

#[async_trait]
impl LoopHook for TurnBudget {
    async fn before_llm(&self, ctx: &mut BeforeLlm<'_>) -> HookAction {
        let turn = ctx.state.turn;
        if turn >= self.max_turns + 2 {
            return HookAction::Halt("subagent turn budget exhausted".into());
        }
        if turn >= self.max_turns && !self.exhausted.swap(true, Ordering::SeqCst) {
            ctx.append_tail(Message::user(
                "Turn limit reached. Do not call any more tools: reply now with a concise summary \
                 of what you found so far and what remains open.",
            ));
        }
        HookAction::Continue
    }

    async fn before_tool(&self, _s: &AgentState, _call: &ToolCall, _ro: bool) -> ToolDecision {
        if self.exhausted.load(Ordering::SeqCst) {
            ToolDecision::Deny {
                reason: "turn limit reached: summarize instead".into(),
            }
        } else {
            ToolDecision::Approve
        }
    }
}

fn assistant_texts(state: &AgentState) -> Vec<String> {
    state
        .messages()
        .iter()
        .filter(|m| m.role == Role::Assistant)
        .map(Message::text)
        .filter(|t| !t.trim().is_empty())
        .collect()
}

/// Runs one subagent to completion (or budget / timeout / cancellation).
pub async fn run_subagent(
    registry: &Arc<Registry>,
    parent: &SubagentRuntime,
    parent_call_id: &str,
    task: SubagentTask,
    cancel: CancellationToken,
    emit: EventFn,
) -> Result<SubagentOutcome, ApiError> {
    let profile = registry.subagent_profile(&task.profile).await?;
    let limits = registry.subagents.clone();
    let depth = parent.lineage.depth + 1;
    let can_spawn = depth < limits.max_depth;

    // The parent's catalog, served by what is live (read-only flags come from live tools).
    let parent_stored = parent.store.load_state(&parent.session_id).await?;
    let parent_tools = parent_stored
        .state
        .tool_snapshot()
        .unwrap_or_default()
        .to_vec();
    let groups = parse_groups(&parent.lineage.builtin_groups)?;
    let mut cfg = parent.cfg.clone();
    if let Some((provider_id, model)) = &profile.model {
        cfg.provider_id = Some(provider_id.clone());
        if !model.is_empty() {
            cfg.model = model.clone();
        }
        if let Ok(p) = registry.config.get_provider(provider_id).await {
            cfg.provider = p.kind.as_str().into();
            if model.is_empty() {
                cfg.model = p.default_model;
            }
        }
    }
    // Child events → relayed to the parent; child tools (nested spawns) use the same path.
    let (ev_tx, mut ev_rx) = mpsc::channel::<AgentEvent>(256);

    let child_runtime = |sid: String| SubagentRuntime {
        session_id: sid,
        cfg: cfg.clone(),
        lineage: Lineage {
            parent_session_id: Some(parent.session_id.clone()),
            parent_tool_call_id: Some(parent_call_id.to_string()),
            depth,
            workspace_root: parent.lineage.workspace_root.clone(),
            builtin_groups: parent.lineage.builtin_groups.clone(),
        },
        store: parent.store.clone(),
        events: ev_tx.clone(),
    };
    let selection = |rt: Option<SubagentRuntime>| ToolSelection {
        builtin: cfg.builtin_tools,
        widgets: cfg.widgets,
        skills: cfg.skills.iter().map(|s| s.name.clone()).collect(),
        mcp: &cfg.mcp_servers,
        groups: groups.clone(),
        workspace_root: parent.lineage.workspace_root.as_ref().map(Into::into),
        runtime: rt,
        depth,
    };
    // Only for read-only flags: no runtime, so it holds no handle on the child's event channel
    // (which must close when the child agent is dropped).
    let probe_live = registry.live_executor(&selection(None), false).await?;
    let parent_view = PinnedToolExecutor::new(&parent_tools, probe_live);

    let (system, initial, frozen_defs) = match task.mode {
        ContextMode::Fork => {
            // Byte-identical prefix: parent system prompt, catalog and history up to the
            // assistant message that issued this call; then the task.
            let msgs = parent_stored.state.messages();
            let cut = msgs
                .iter()
                .rposition(|m| {
                    m.role == Role::Assistant
                        && m.content.iter().any(
                            |p| matches!(p, ContentBlock::ToolCall(c) if c.id == parent_call_id),
                        )
                })
                .unwrap_or(msgs.len());
            let mut initial = msgs[..cut].to_vec();
            initial.push(Message::user(format!(
                "[Subagent task — you are now a read-only subagent ({}); report back instead of asking the user]\n\n{}{}",
                profile.name,
                task.task,
                task.context.as_ref().map(|c| format!("\n\nContext:\n{c}")).unwrap_or_default()
            )));
            (
                parent_stored.state.system_prompt().to_string(),
                initial,
                parent_tools.clone(),
            )
        }
        ContextMode::Fresh => {
            let wanted: Option<BTreeSet<&str>> = task
                .tools
                .as_ref()
                .map(|t| t.iter().map(String::as_str).collect());
            let defs: Vec<ToolDefinition> = parent_view
                .tools()
                .iter()
                .filter(|t| t.is_read_only())
                .map(|t| t.definition())
                .filter(|d| {
                    d.name != oort_tools::ASK_USER
                        && d.name != crate::session_tools::RENDER_WIDGET
                        && d.name != "update_plan"
                        && (d.name != SPAWN_SUBAGENT || can_spawn)
                        && wanted.as_ref().is_none_or(|w| w.contains(d.name.as_str()))
                })
                .collect();
            let mut text = task.task.clone();
            if let Some(c) = &task.context {
                text.push_str(&format!("\n\nContext from the parent agent:\n{c}"));
            }
            (profile.prompt.clone(), vec![Message::user(text)], defs)
        }
    };

    // Freeze through a pinned view so the stored catalog is exactly `frozen_defs`, sorted.
    let pinned_for_freeze =
        PinnedToolExecutor::new(&frozen_defs, Arc::new(oort_agent::LocalToolExecutor::new()));
    let frozen =
        FrozenToolset::freeze(&pinned_for_freeze).map_err(|e| ApiError::Internal(e.to_string()))?;
    let state = AgentState::with_tools(system, initial, &frozen);
    let meta = NewSession {
        id: None,
        title: format!(
            "{} · {}",
            profile.name,
            task.task.chars().take(48).collect::<String>()
        ),
        title_source: Default::default(),
        provider: cfg.provider.clone(),
        model: cfg.model.clone(),
        config: serde_json::to_value(&cfg).map_err(|e| ApiError::Internal(e.to_string()))?,
        approval_policy: ApprovalPolicyKind::AutoAll,
        lineage: child_runtime(String::new()).lineage,
        reasoning_effort: Default::default(),
        enabled_server_tools: None,
    };
    let sub_id = parent.store.create_session(meta, &state).await?;
    drop(parent_view);

    // The live executor bound to the child session (its own spawns relay through it).
    let live = registry
        .live_executor(&selection(Some(child_runtime(sub_id.clone()))), false)
        .await?;
    let tools = Arc::new(PinnedToolExecutor::new(&frozen_defs, live));
    let model = registry.session_model(&cfg).await?;
    let defaults = registry.provider_defaults(&cfg).await;
    let thinking = Arc::new(std::sync::RwLock::new(defaults.reasoning_effort.thinking()));
    let agent = Agent::builder(model, tools)
        .context_manager(Arc::new(
            cfg.context_manager(&sub_id, &defaults, thinking, None),
        ))
        .hook(Arc::new(TracingHook))
        .hook(Arc::new(SubagentGuard { can_spawn }))
        .hook(Arc::new(TurnBudget {
            max_turns: profile.max_turns,
            exhausted: AtomicBool::new(false),
        }))
        .hook(Arc::new(PersistenceHook::new(
            parent.store.clone(),
            sub_id.clone(),
            &state,
        )))
        .events(ev_tx.clone())
        .session_cancel(cancel.clone())
        .build()
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    drop(ev_tx);

    let relay_profile = profile.name.clone();
    let relay_call = parent_call_id.to_string();
    let relay_sub = sub_id.clone();
    let relay = tokio::spawn(async move {
        while let Some(ev) = ev_rx.recv().await {
            emit(AgentEvent::Subagent {
                tool_call_id: relay_call.clone(),
                sub_session_id: relay_sub.clone(),
                profile: relay_profile.clone(),
                inner: Box::new(ev),
            });
        }
    });

    let mut state = state;
    let mut inbox = Inbox::detached();
    let run = tokio::time::timeout(limits.timeout, agent.run_loop(&mut state, &mut inbox)).await;
    drop(agent);
    let _ = relay.await;
    let texts = assistant_texts(&state);
    let (ending, summary) = match run {
        Ok(Ok(())) if state.turn > profile.max_turns => ("turn_limit", texts.last().cloned()),
        Ok(Ok(())) => ("completed", texts.last().cloned()),
        Ok(Err(oort_agent::AgentError::Cancelled)) => {
            return Err(ApiError::Conflict("subagent cancelled".into()));
        }
        Ok(Err(e)) => return Err(ApiError::BadGateway(format!("subagent failed: {e}"))),
        Err(_elapsed) => {
            cancel.cancel();
            (
                "timeout",
                Some(texts.join("\n\n")).filter(|t| !t.is_empty()),
            )
        }
    };
    let summary = summary.unwrap_or_else(|| "(the subagent produced no report)".into());
    let summary = match ending {
        "timeout" => format!(
            "[timed out after {}s — partial findings]\n\n{summary}",
            limits.timeout.as_secs()
        ),
        "turn_limit" => format!("[turn limit reached — summary]\n\n{summary}"),
        _ => summary,
    };
    Ok(SubagentOutcome {
        sub_session_id: sub_id,
        summary,
        ending,
    })
}

// ----- the tool --------------------------------------------------------------------------------

pub struct SpawnSubagent {
    pub registry: Weak<Registry>,
    pub runtime: Option<SubagentRuntime>,
}

impl Tool for SpawnSubagent {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new(
            SPAWN_SUBAGENT,
            "Delegate a self-contained, read-only task (exploring, researching, reviewing) to an isolated \
             subagent; only its final report comes back. Several calls in one turn run in parallel. \
             Subagents cannot change anything or ask the user.",
            json!({
                "type": "object",
                "properties": {
                    "task": { "type": "string", "description": "what to find out or check, self-contained" },
                    "context": { "type": "string", "description": "only the context it needs (fresh mode)" },
                    "profile": { "type": "string", "description": "explorer (default), researcher, reviewer, or preset:<name>" },
                    "context_mode": { "type": "string", "enum": ["fresh", "fork"], "description": "fresh (default): a clean context; fork: continue from this conversation" },
                    "tools": { "type": "array", "items": { "type": "string" }, "description": "restrict to these tool names (fresh mode)" }
                },
                "required": ["task"]
            }),
        )
    }

    /// Subagents are read-only, so they may run concurrently with each other.
    fn is_read_only(&self) -> bool {
        true
    }
}

pub fn parse_task(args: &Value) -> Result<SubagentTask, ToolError> {
    let s = |k: &str| {
        args.get(k)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|v| !v.is_empty())
    };
    Ok(SubagentTask {
        task: s("task")
            .ok_or_else(|| ToolError::InvalidArguments("missing string argument `task`".into()))?
            .to_string(),
        context: s("context").map(str::to_string),
        profile: s("profile").unwrap_or("explorer").to_string(),
        mode: match s("context_mode") {
            None | Some("fresh") => ContextMode::Fresh,
            Some("fork") => ContextMode::Fork,
            Some(o) => {
                return Err(ToolError::InvalidArguments(format!(
                    "context_mode must be fresh or fork, not {o}"
                )));
            }
        },
        tools: args.get("tools").and_then(Value::as_array).map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect()
        }),
    })
}

#[async_trait]
impl LocalTool for SpawnSubagent {
    async fn run(&self, args: Value, ctx: ToolContext) -> Result<ToolOutput, ToolError> {
        let task = parse_task(&args)?;
        let registry = self
            .registry
            .upgrade()
            .ok_or_else(|| ToolError::Failed("server is shutting down".into()))?;
        let rt = self
            .runtime
            .clone()
            .ok_or_else(|| ToolError::Failed("subagents are not available here".into()))?;
        let events = rt.events.clone();
        let emit: EventFn = Arc::new(move |ev| {
            let _ = events.try_send(ev);
        });
        let profile = task.profile.clone();
        // Bound to this call: interrupting / stopping the parent cancels the child tree.
        let cancel = ctx.cancel.child_token();
        let out = tokio::select! {
            r = run_subagent(&registry, &rt, &ctx.call_id, task, cancel.clone(), emit) => r,
            _ = ctx.cancel.cancelled() => {
                cancel.cancel();
                return Err(ToolError::Cancelled);
            }
        };
        match out {
            Ok(o) => Ok(ToolOutput::ok(format!(
                "{}\n\n[subagent: {profile} · session {} · {}]",
                o.summary, o.sub_session_id, o.ending
            ))),
            Err(ApiError::Conflict(m)) if m.contains("cancelled") => Err(ToolError::Cancelled),
            Err(e) => Err(ToolError::Failed(e.to_string())),
        }
    }
}
