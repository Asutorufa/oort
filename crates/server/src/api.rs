//! JSON contract of the HTTP API (mirrored by `web/src/types.ts`).

use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use oort_agent::{
    AgentEvent, AgentState, ApprovalDecision, Phase, SessionMeta, StoreError, ToolDecision,
    UsageLedger,
};
use oort_llm::{ContentPart, Message, Role, ToolCall, ToolDefinition, Usage};
use serde::{Deserialize, Serialize};

use crate::config::{ApprovalMode, ProviderKind, SessionConfig};

// ----- requests ------------------------------------------------------------------------------

#[derive(Debug, Clone, Deserialize)]
pub struct CreateSessionRequest {
    #[serde(default)]
    pub title: Option<String>,
    pub provider: ProviderKind,
    pub model: String,
    #[serde(default)]
    pub system_prompt: String,
    #[serde(default)]
    pub max_context_tokens: Option<u64>,
    #[serde(default)]
    pub max_tokens: Option<u32>,
    #[serde(default)]
    pub approval: ApprovalMode,
    /// Optional first user message (starts the first turn immediately).
    #[serde(default)]
    pub message: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum InputRequest {
    /// Soft insert (also how a new message is sent to an idle session).
    Steer {
        text: String,
    },
    /// Hard interrupt of the current turn.
    Interrupt {
        text: String,
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
    pub settings: SessionConfig,
    pub phase: Phase,
    pub system_prompt: String,
    pub tools: Vec<ToolDefinition>,
    pub pending_approvals: Vec<PendingApproval>,
    pub pending_steers: usize,
    pub raw: Vec<RawMessage>,
    pub checkpoints: Vec<CheckpointView>,
    pub active_view: ActiveView,
    pub usage: UsageView,
    pub context: ContextView,
}

impl SessionDetail {
    pub fn build(
        meta: SessionMeta,
        active: bool,
        settings: SessionConfig,
        state: &AgentState,
        usage: &[Option<Usage>],
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
            per_turn.push(TurnUsage {
                turn: per_turn.len() + 1,
                seq,
                input_tokens: u.input_tokens,
                output_tokens: u.output_tokens,
                cache_read_tokens: c,
                cache_creation_tokens: u.cache_creation_tokens.unwrap_or(0),
                prompt_tokens: p,
                cache_hit_rate: ratio(c, p),
            });
        }

        let pending_approvals = match &state.phase {
            Phase::AwaitingApproval { calls, decisions } => calls
                .iter()
                .filter_map(|c| match decisions.get(&c.id) {
                    Some(ToolDecision::NeedHumanApproval { reason }) => Some(PendingApproval {
                        call: c.clone(),
                        reason: reason.clone(),
                    }),
                    _ => None,
                })
                .collect(),
            _ => vec![],
        };

        Self {
            active,
            phase: state.phase.clone(),
            system_prompt: state.system_prompt().to_string(),
            tools: state.tool_snapshot().map(<[_]>::to_vec).unwrap_or_default(),
            pending_approvals,
            pending_steers: state.pending_steers.len(),
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

/// One SSE message (`data:` is this JSON; `id:` is `seq`).
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Envelope {
    Agent {
        seq: u64,
        event: AgentEvent,
    },
    /// The background loop stopped (shutdown, cancel, halt, or a fatal error).
    LoopExited {
        seq: u64,
        error: Option<String>,
    },
    /// The subscriber fell behind and missed events: refetch the snapshot.
    Lagged {
        skipped: u64,
    },
}

impl Envelope {
    pub fn seq(&self) -> Option<u64> {
        match self {
            Self::Agent { seq, .. } | Self::LoopExited { seq, .. } => Some(*seq),
            Self::Lagged { .. } => None,
        }
    }
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
            Self::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
        };
        (
            status,
            Json(serde_json::json!({ "error": self.to_string() })),
        )
            .into_response()
    }
}
