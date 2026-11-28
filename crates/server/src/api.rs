//! JSON contract of the HTTP API (mirrored by `web/src/types.ts`).

use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use oort_agent::{
    AgentEvent, AgentState, ApprovalDecision, ApprovalPolicyKind, LlmCallStats, Phase,
    RequestSummary, SessionMeta, SessionStatus, StoreError, ToolDecision, UsageLedger,
};
use oort_llm::{ContentPart, Message, Role, ToolCall, ToolDefinition, Usage};
use serde::{Deserialize, Serialize};

use crate::config::SessionConfig;
use crate::hub::LiveTurn;

// ----- requests ------------------------------------------------------------------------------

/// `POST /api/sessions`. Fields left out come from the preset (if any), then defaults.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct CreateSessionRequest {
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub preset_id: Option<String>,
    #[serde(default)]
    pub provider_id: Option<String>,
    /// Empty: the preset's model, then the provider's default model.
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub mcp_server_ids: Option<Vec<String>>,
    #[serde(default)]
    pub builtin_tools: Option<bool>,
    #[serde(default)]
    pub system_prompt: Option<String>,
    #[serde(default)]
    pub approval_policy: Option<ApprovalPolicyKind>,
    #[serde(default)]
    pub max_context_tokens: Option<u64>,
    #[serde(default)]
    pub max_tokens: Option<u32>,
    /// Thinking level (`default`: the preset's, then the provider's).
    #[serde(default)]
    pub reasoning_effort: Option<oort_agent::ReasoningEffort>,
    /// Skill ids (DB ids or `file:<name>`) whose index is frozen into the session.
    #[serde(default)]
    pub skill_ids: Option<Vec<String>>,
    /// Builtin tool groups (`self_config`, `core_ux`, `memory`, `utilities`, `subagent`,
    /// `workspace`).
    #[serde(default)]
    pub builtin_groups: Option<Vec<String>>,
    /// Required with the `workspace` group: an existing directory.
    #[serde(default)]
    pub workspace_root: Option<String>,
    /// Offer `render_widget` (default true).
    #[serde(default)]
    pub widgets: Option<bool>,
    /// Optional first user message (starts the first turn immediately).
    #[serde(default)]
    pub message: Option<String>,
}

/// `PATCH /api/sessions/:id`.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct PatchSessionRequest {
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub approval_policy: Option<ApprovalPolicyKind>,
    /// Changes the thinking level from the next request on (history is untouched).
    #[serde(default)]
    pub reasoning_effort: Option<oort_agent::ReasoningEffort>,
    /// Switches the model (same provider) from the next request on.
    #[serde(default)]
    pub model: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct InvokedSkill {
    pub name: String,
    #[serde(default)]
    pub arguments: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum InputRequest {
    /// Soft insert (also how a new message is sent to an idle session).
    Steer {
        text: String,
        /// Skills attached explicitly (chips / `/name`), appended to this message.
        #[serde(default)]
        invoked_skills: Vec<InvokedSkill>,
    },
    /// Hard interrupt of the current turn.
    Interrupt {
        text: String,
        #[serde(default)]
        invoked_skills: Vec<InvokedSkill>,
    },
    Approval {
        call_id: String,
        #[serde(flatten)]
        decision: ApprovalDecision,
    },
    Compact,
}

// ----- responses -----------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
pub struct SessionSummary {
    #[serde(flatten)]
    pub meta: SessionMeta,
    pub active: bool,
    /// The last loop exited with an error (details in the session detail's `last_error`).
    pub failed: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct RawMessage {
    pub seq: usize,
    pub role: Role,
    pub content: Vec<ContentPart>,
    pub usage: Option<Usage>,
}

#[derive(Debug, Clone, Serialize)]
pub struct CheckpointView {
    pub epoch: usize,
    pub up_to_seq: usize,
    pub summary: Message,
}

#[derive(Debug, Clone, Serialize)]
pub struct PendingApproval {
    pub call: ToolCall,
    pub reason: String,
    /// `approval` (approve / deny) or `input` (answer with `decision: "answer"`).
    pub kind: &'static str,
    /// What the card shows: a diff, the command, the skill or server settings.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub preview: Option<serde_json::Value>,
}

/// Usage of one LLM call, normalized across providers.
#[derive(Debug, Clone, Serialize)]
pub struct TurnUsage {
    pub turn: usize,
    /// Seq of the assistant message this call produced.
    pub seq: usize,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_creation_tokens: u64,
    /// All prompt tokens (cached or not), whatever the provider's accounting.
    pub prompt_tokens: u64,
    /// `cache_read_tokens / prompt_tokens`.
    pub cache_hit_rate: f64,
    /// Time to first streamed content (calls made before metrics existed have none).
    pub ttft_ms: Option<u64>,
    pub latency_ms: Option<u64>,
    pub request: Option<RequestSummary>,
}

#[derive(Debug, Clone, Serialize)]
pub struct UsageView {
    pub ledger: UsageLedger,
    pub per_turn: Vec<TurnUsage>,
    /// Σ cache_read / Σ prompt over all turns.
    pub cache_hit_rate: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct ContextView {
    /// Prompt tokens of the latest call in the current epoch (`None` right after compaction).
    pub tokens: Option<u64>,
    pub max_tokens: u64,
    pub compaction_threshold: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct ActiveView {
    /// Raw seq where the uncompacted tail starts (0 without compaction).
    pub from_seq: usize,
    pub messages: Vec<Message>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SessionDetail {
    #[serde(flatten)]
    pub meta: SessionMeta,
    pub active: bool,
    /// Why the loop last stopped with an error (an LLM 429, a network failure, …). Cleared when
    /// the loop starts again; while it is set the session is not resumed automatically.
    pub last_error: Option<LoopError>,
    /// The thinking level requests use: the session's, or the provider's when it is `default`.
    pub effective_reasoning_effort: oort_agent::ReasoningEffort,
    pub settings: SessionConfig,
    pub phase: Phase,
    pub system_prompt: String,
    pub tools: Vec<ToolDefinition>,
    pub pending_approvals: Vec<PendingApproval>,
    pub pending_steers: usize,
    /// Skills whose body is loaded in the current epoch (explicit invocation).
    pub loaded_skills: Vec<String>,
    pub raw: Vec<RawMessage>,
    pub checkpoints: Vec<CheckpointView>,
    pub active_view: ActiveView,
    pub usage: UsageView,
    pub context: ContextView,
}

/// A loop that exited with an error, kept so clients can show it and offer a retry.
#[derive(Debug, Clone, Serialize)]
pub struct LoopError {
    pub message: String,
    /// Unix milliseconds.
    pub at: i64,
    /// When the loop is retried automatically (provider setting), in Unix milliseconds.
    pub retry_at: Option<i64>,
    /// Automatic retries already made for this failure streak, and the provider's limit.
    pub attempt: u32,
    pub max_attempts: u32,
}

impl SessionDetail {
    pub fn build(
        meta: SessionMeta,
        active: bool,
        settings: SessionConfig,
        state: &AgentState,
        usage: &[Option<Usage>],
        calls: &[LlmCallStats],
    ) -> Self {
        let policy = settings.policy();
        let raw = state
            .messages()
            .iter()
            .enumerate()
            .map(|(seq, m)| RawMessage {
                seq,
                role: m.role,
                content: m.content.clone(),
                usage: usage.get(seq).cloned().flatten(),
            })
            .collect();

        let mut per_turn = Vec::new();
        let (mut cached, mut prompt) = (0u64, 0u64);
        for (seq, u) in usage.iter().enumerate() {
            let Some(u) = u else { continue };
            let p = policy.context_tokens(u);
            let c = u.cache_read_tokens.unwrap_or(0);
            cached += c;
            prompt += p;
            let call = calls.iter().find(|s| s.seq == seq);
            per_turn.push(TurnUsage {
                turn: per_turn.len() + 1,
                seq,
                input_tokens: u.input_tokens,
                output_tokens: u.output_tokens,
                cache_read_tokens: c,
                cache_creation_tokens: u.cache_creation_tokens.unwrap_or(0),
                prompt_tokens: p,
                cache_hit_rate: ratio(c, p),
                ttft_ms: call.and_then(|s| s.ttft_ms),
                latency_ms: call.map(|s| s.latency_ms),
                request: call.map(|s| s.request.clone()),
            });
        }

        let pending_approvals = match &state.phase {
            Phase::AwaitingApproval { calls, decisions } => calls
                .iter()
                .filter_map(|c| match decisions.get(&c.id) {
                    Some(ToolDecision::NeedHumanApproval { reason }) => Some(PendingApproval {
                        call: c.clone(),
                        reason: reason.clone(),
                        kind: "approval",
                        preview: None,
                    }),
                    Some(ToolDecision::NeedUserInput { reason }) => Some(PendingApproval {
                        call: c.clone(),
                        reason: reason.clone(),
                        kind: "input",
                        preview: None,
                    }),
                    _ => None,
                })
                .collect(),
            _ => vec![],
        };

        Self {
            active,
            last_error: None,
            effective_reasoning_effort: Default::default(),
            phase: state.phase.clone(),
            system_prompt: state.system_prompt().to_string(),
            tools: state.tool_snapshot().map(<[_]>::to_vec).unwrap_or_default(),
            pending_approvals,
            pending_steers: state.pending_steers.len(),
            loaded_skills: state.loaded_skills.iter().cloned().collect(),
            raw,
            checkpoints: state
                .checkpoints()
                .iter()
                .enumerate()
                .map(|(i, c)| CheckpointView {
                    epoch: i + 1,
                    up_to_seq: c.up_to_index,
                    summary: c.summary_message.clone(),
                })
                .collect(),
            active_view: ActiveView {
                from_seq: state.latest_checkpoint().map_or(0, |c| c.up_to_index),
                messages: state.active_view(),
            },
            usage: UsageView {
                ledger: state.usage.clone(),
                per_turn,
                cache_hit_rate: ratio(cached, prompt),
            },
            context: ContextView {
                tokens: state.usage.last.as_ref().map(|u| policy.context_tokens(u)),
                max_tokens: policy.max_context_tokens,
                compaction_threshold: (policy.max_context_tokens as f64 * policy.trigger_ratio)
                    as u64,
            },
            settings,
            meta,
        }
    }
}

fn ratio(a: u64, b: u64) -> f64 {
    if b == 0 { 0.0 } else { a as f64 / b as f64 }
}

/// One SSE message. `data:` is this JSON; `id:` is `{boot}:{seq}` so a reconnecting client
/// (`Last-Event-ID`) can resume within the same server process.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Envelope {
    /// First message of every connection. `resumed`: missed events follow and the client
    /// keeps its state; otherwise it must refetch the snapshot and adopt `live`.
    Hello {
        seq: u64,
        boot: String,
        resumed: bool,
        status: SessionStatus,
        active: bool,
        live: LiveTurn,
    },
    Agent {
        seq: u64,
        event: AgentEvent,
    },
    /// The background loop stopped (shutdown, cancel, halt, or a fatal error).
    LoopExited {
        seq: u64,
        error: Option<String>,
    },
    /// A `render_widget` call was committed (`widget` is its arguments; also in the history).
    Widget {
        seq: u64,
        call_id: String,
        widget: serde_json::Value,
    },
    /// Title or approval policy changed.
    SessionUpdated {
        seq: u64,
        title: String,
        approval_policy: ApprovalPolicyKind,
        reasoning_effort: oort_agent::ReasoningEffort,
        model: String,
    },
    /// The session was deleted; the stream ends.
    Deleted {
        seq: u64,
    },
    /// The subscriber fell behind and missed events: refetch the snapshot.
    Lagged {
        skipped: u64,
    },
}

impl Envelope {
    pub fn seq(&self) -> Option<u64> {
        match self {
            Self::Hello { seq, .. }
            | Self::Agent { seq, .. }
            | Self::LoopExited { seq, .. }
            | Self::Widget { seq, .. }
            | Self::SessionUpdated { seq, .. }
            | Self::Deleted { seq } => Some(*seq),
            Self::Lagged { .. } => None,
        }
    }
}

/// `GET /api/sessions/:id/export` (no secrets).
#[derive(Debug, Clone, Serialize)]
pub struct SessionExport {
    pub format: &'static str,
    pub exported_at: i64,
    pub meta: SessionMeta,
    pub settings: SessionConfig,
    pub provider: Option<ProviderSummary>,
    pub system_prompt: String,
    pub frozen_tools: Vec<ToolDefinition>,
    pub messages: Vec<RawMessage>,
    pub compactions: Vec<CheckpointView>,
    pub llm_calls: Vec<LlmCallStats>,
    pub phase: Phase,
    pub pending_steers: Vec<Message>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ProviderSummary {
    pub id: String,
    pub name: String,
    pub kind: oort_agent::ProviderKind,
    pub base_url: Option<String>,
}

// ----- errors --------------------------------------------------------------------------------

#[derive(Debug, thiserror::Error)]
pub enum ApiError {
    #[error("not found: {0}")]
    NotFound(String),
    #[error("{0}")]
    BadRequest(String),
    #[error("{0}")]
    Conflict(String),
    /// An upstream (LLM provider or MCP server) failed.
    #[error("{0}")]
    BadGateway(String),
    #[error("{0}")]
    Internal(String),
}

impl From<StoreError> for ApiError {
    fn from(e: StoreError) -> Self {
        match e {
            StoreError::NotFound(id) => Self::NotFound(id),
            StoreError::Conflict(m) => Self::Conflict(m),
            other => Self::Internal(other.to_string()),
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let status = match self {
            Self::NotFound(_) => StatusCode::NOT_FOUND,
            Self::BadRequest(_) => StatusCode::BAD_REQUEST,
            Self::Conflict(_) => StatusCode::CONFLICT,
            Self::BadGateway(_) => StatusCode::BAD_GATEWAY,
            Self::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
        };
        (
            status,
            Json(serde_json::json!({ "error": self.to_string() })),
        )
            .into_response()
    }
}
