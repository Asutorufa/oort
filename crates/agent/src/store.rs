//! Database-agnostic session persistence.
//!
//! The append-only design maps directly onto storage: a step only ever *appends* messages and
//! checkpoints, and rewrites a small session row (status, queued steers, usage). A
//! [`SessionJournal`] tracks what has been written and turns each step into a [`StepDelta`],
//! which backends commit atomically, so stored sessions are always at a step boundary
//! (never with half-answered tool calls).

use std::collections::{BTreeMap, VecDeque};
use std::sync::Arc;

use async_trait::async_trait;
use oort_llm::{Message, ToolCall, ToolDefinition, Usage};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::sync::Mutex;

use crate::config_store::ApprovalPolicyKind;
use crate::error::AgentError;
use crate::hook::LoopHook;
use crate::metrics::LlmCallStats;
use crate::state::{
    AgentState, CompactionCheckpoint, Phase, StateParts, ToolDecision, UsageLedger,
};

pub type SessionId = String;

/// Coarse session status, as stored and listed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionStatus {
    Idle,
    /// `Ready` or `ToolsPending`: there is work to do (also what a crashed session looks like).
    Running,
    NeedsApproval,
    Halted,
}

impl SessionStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Running => "running",
            Self::NeedsApproval => "needs_approval",
            Self::Halted => "halted",
        }
    }

    pub fn parse(s: &str) -> Result<Self, StoreError> {
        Ok(match s {
            "idle" => Self::Idle,
            "running" => Self::Running,
            "needs_approval" => Self::NeedsApproval,
            "halted" => Self::Halted,
            other => return Err(StoreError::Corrupt(format!("unknown status {other:?}"))),
        })
    }
}

/// Tool calls awaiting execution. `decisions` is `None` before hooks ran (`ToolsPending`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PendingCalls {
    pub calls: Vec<ToolCall>,
    pub decisions: Option<BTreeMap<String, ToolDecision>>,
}

/// The mutable part of a session (everything except the append-only logs).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionSnapshot {
    pub status: SessionStatus,
    pub halt_reason: Option<String>,
    pub pending_calls: Option<PendingCalls>,
    pub pending_steers: Vec<Message>,
    pub compaction_requested: bool,
    pub turn: u32,
    pub usage: UsageLedger,
    #[serde(default)]
    pub loaded_skills: Vec<String>,
}

impl SessionSnapshot {
    pub fn from_state(state: &AgentState) -> Self {
        let (status, halt_reason, pending_calls) = match &state.phase {
            Phase::Ready => (SessionStatus::Running, None, None),
            Phase::ToolsPending { calls } => (
                SessionStatus::Running,
                None,
                Some(PendingCalls {
                    calls: calls.clone(),
                    decisions: None,
                }),
            ),
            Phase::AwaitingApproval { calls, decisions } => (
                SessionStatus::NeedsApproval,
                None,
                Some(PendingCalls {
                    calls: calls.clone(),
                    decisions: Some(decisions.clone()),
                }),
            ),
            Phase::Idle => (SessionStatus::Idle, None, None),
            Phase::Halted { reason } => (SessionStatus::Halted, Some(reason.clone()), None),
        };
        Self {
            status,
            halt_reason,
            pending_calls,
            pending_steers: state.pending_steers.iter().cloned().collect(),
            compaction_requested: state.compaction_requested,
            turn: state.turn,
            usage: state.usage.clone(),
            loaded_skills: state.loaded_skills.iter().cloned().collect(),
        }
    }

    /// Inverse of the phase mapping in [`Self::from_state`].
    pub fn phase(&self) -> Result<Phase, StoreError> {
        let corrupt = |m: &str| Err(StoreError::Corrupt(m.into()));
        Ok(match (self.status, &self.pending_calls) {
            (SessionStatus::Running, None) => Phase::Ready,
            (
                SessionStatus::Running,
                Some(PendingCalls {
                    calls,
                    decisions: None,
                }),
            ) => Phase::ToolsPending {
                calls: calls.clone(),
            },
            (
                SessionStatus::NeedsApproval,
                Some(PendingCalls {
                    calls,
                    decisions: Some(d),
                }),
            ) => Phase::AwaitingApproval {
                calls: calls.clone(),
                decisions: d.clone(),
            },
            (SessionStatus::Idle, None) => Phase::Idle,
            (SessionStatus::Halted, None) => Phase::Halted {
                reason: self.halt_reason.clone().unwrap_or_default(),
            },
            _ => return corrupt("status and pending_calls are inconsistent"),
        })
    }
}

/// Parameters for a new session row.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct NewSession {
    /// Explicit id; the backend generates one when `None`.
    pub id: Option<SessionId>,
    pub title: String,
    pub provider: String,
    pub model: String,
    /// Immutable, application-defined session configuration (request parameters, policies, …).
    /// It is part of what keeps a resumed session's requests byte-identical.
    #[serde(default)]
    pub config: serde_json::Value,
    /// Mutable later via [`SessionStore::set_approval_policy`].
    #[serde(default)]
    pub approval_policy: ApprovalPolicyKind,
    #[serde(default)]
    pub lineage: Lineage,
}

/// Where a session sits in a subagent tree, and its immutable tool environment.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Lineage {
    /// Set for subagent sessions (hidden from the top-level list).
    pub parent_session_id: Option<SessionId>,
    /// The parent's tool call that spawned it (`skill:<id>` for skill-run subagents).
    pub parent_tool_call_id: Option<String>,
    pub depth: u32,
    /// Root directory of the workspace tools (only when the `workspace` group is enabled).
    pub workspace_root: Option<String>,
    /// Builtin tool groups enabled at creation.
    pub builtin_groups: Vec<String>,
}

/// Session metadata (one row of `sessions`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionMeta {
    pub id: SessionId,
    pub title: String,
    pub provider: String,
    pub model: String,
    pub config: serde_json::Value,
    pub approval_policy: ApprovalPolicyKind,
    pub lineage: Lineage,
    pub status: SessionStatus,
    pub turn: u32,
    pub message_count: u64,
    /// Unix milliseconds.
    pub created_at: i64,
    pub updated_at: i64,
}

/// A fully loaded session.
#[derive(Debug, Clone)]
pub struct StoredSession {
    pub meta: SessionMeta,
    pub state: AgentState,
    /// `message_usage[seq]`: usage of the LLM call that produced that message (assistant only).
    pub message_usage: Vec<Option<Usage>>,
}

/// What one step changed.
#[derive(Debug, Clone, PartialEq)]
pub struct StepDelta {
    /// Seq of `messages[0]`; equals the number of messages already stored.
    pub first_seq: u64,
    pub messages: Vec<(Message, Option<Usage>)>,
    /// 1-based epoch of `checkpoints[0]`.
    pub first_epoch: u32,
    pub checkpoints: Vec<CompactionCheckpoint>,
    /// Measurements of the LLM calls made in this step (keyed by the produced message's seq).
    pub llm_calls: Vec<LlmCallStats>,
    pub snapshot: SessionSnapshot,
}

impl StepDelta {
    pub fn is_empty(&self) -> bool {
        self.messages.is_empty() && self.checkpoints.is_empty()
    }
}

#[derive(Debug, Error)]
pub enum StoreError {
    #[error("session not found: {0}")]
    NotFound(SessionId),
    /// An optimistic check failed (e.g. a seq was already written by someone else).
    #[error("conflict: {0}")]
    Conflict(String),
    /// Stored data violates an invariant.
    #[error("corrupt session data: {0}")]
    Corrupt(String),
    #[error("storage backend error: {0}")]
    Backend(String),
}

impl From<StoreError> for AgentError {
    fn from(e: StoreError) -> Self {
        AgentError::Storage(e.to_string())
    }
}

impl From<AgentError> for StoreError {
    fn from(e: AgentError) -> Self {
        StoreError::Corrupt(e.to_string())
    }
}

/// Canonical (deterministic) JSON for the frozen tool catalog. Stored byte-for-byte and
/// re-checked on load so a resumed session sends the exact same tool prefix.
pub fn canonical_tools_json(tools: &[ToolDefinition]) -> String {
    serde_json::to_string(tools).expect("tool definitions are always serializable")
}

/// Session persistence backend.
#[async_trait]
pub trait SessionStore: Send + Sync {
    /// Creates the session row and stores the state's existing messages/checkpoints.
    /// The state's tool catalog must already be frozen (see `Agent::new_state`).
    async fn create_session(
        &self,
        meta: NewSession,
        state: &AgentState,
    ) -> Result<SessionId, StoreError>;

    /// Top-level sessions (no parent), most recently updated first.
    async fn list_sessions(&self) -> Result<Vec<SessionMeta>, StoreError>;

    /// Direct children (subagent sessions) of `id`, oldest first.
    async fn list_children(&self, id: &str) -> Result<Vec<SessionMeta>, StoreError>;

    async fn load_state(&self, id: &str) -> Result<StoredSession, StoreError>;

    /// Appends `messages[seq]`. `seq` must equal the number of stored messages.
    async fn append_message(
        &self,
        id: &str,
        seq: u64,
        message: &Message,
        usage: Option<&Usage>,
    ) -> Result<(), StoreError>;

    /// Appends checkpoint number `epoch` (1-based, contiguous).
    async fn save_compaction(
        &self,
        id: &str,
        epoch: u32,
        checkpoint: &CompactionCheckpoint,
    ) -> Result<(), StoreError>;

    async fn update_session_status(
        &self,
        id: &str,
        snapshot: &SessionSnapshot,
    ) -> Result<(), StoreError>;

    /// Appends the measurements of one LLM call (its assistant message must be stored).
    async fn save_llm_call(&self, id: &str, stats: &LlmCallStats) -> Result<(), StoreError>;

    /// LLM call measurements in seq order.
    async fn list_llm_calls(&self, id: &str) -> Result<Vec<LlmCallStats>, StoreError>;

    async fn set_title(&self, id: &str, title: &str) -> Result<(), StoreError>;

    /// The approval policy is not part of the request prefix, so it may change at any time.
    async fn set_approval_policy(
        &self,
        id: &str,
        policy: ApprovalPolicyKind,
    ) -> Result<(), StoreError>;

    /// Removes the session and all its logs. The only way history is ever deleted.
    async fn delete_session(&self, id: &str) -> Result<(), StoreError>;

    /// Applies a whole step. Backends with transactions should override this to make it atomic.
    async fn commit_step(&self, id: &str, delta: &StepDelta) -> Result<(), StoreError> {
        for (i, (msg, usage)) in delta.messages.iter().enumerate() {
            self.append_message(id, delta.first_seq + i as u64, msg, usage.as_ref())
                .await?;
        }
        for (i, cp) in delta.checkpoints.iter().enumerate() {
            self.save_compaction(id, delta.first_epoch + i as u32, cp)
                .await?;
        }
        for stats in &delta.llm_calls {
            self.save_llm_call(id, stats).await?;
        }
        self.update_session_status(id, &delta.snapshot).await
    }
}

/// Rebuilds an [`AgentState`] from stored pieces (shared by backends).
pub fn assemble_state(
    system_prompt: String,
    frozen_tools_json: &str,
    messages: Vec<Message>,
    checkpoints: Vec<CompactionCheckpoint>,
    snapshot: SessionSnapshot,
) -> Result<AgentState, StoreError> {
    let tools: Vec<ToolDefinition> = serde_json::from_str(frozen_tools_json)
        .map_err(|e| StoreError::Corrupt(format!("frozen_tools: {e}")))?;
    if canonical_tools_json(&tools) != frozen_tools_json {
        return Err(StoreError::Corrupt(
            "frozen_tools is not in canonical form".into(),
        ));
    }
    let phase = snapshot.phase()?;
    Ok(AgentState::from_parts(StateParts {
        system_prompt,
        tool_snapshot: Some(tools),
        messages,
        checkpoints,
        phase,
        pending_steers: VecDeque::from(snapshot.pending_steers),
        compaction_requested: snapshot.compaction_requested,
        turn: snapshot.turn,
        usage: snapshot.usage,
        loaded_skills: snapshot.loaded_skills.into_iter().collect(),
    })?)
}

/// Tracks how much of an [`AgentState`] is already persisted and produces [`StepDelta`]s.
#[derive(Debug, Clone)]
pub struct SessionJournal {
    messages: usize,
    checkpoints: usize,
    llm_calls: u64,
    last_snapshot: Option<SessionSnapshot>,
}

impl SessionJournal {
    /// A journal for a state that is fully persisted (just created or just loaded).
    pub fn new(state: &AgentState) -> Self {
        Self {
            messages: state.messages().len(),
            checkpoints: state.checkpoints().len(),
            llm_calls: state.usage.llm_calls,
            last_snapshot: Some(SessionSnapshot::from_state(state)),
        }
    }

    /// Everything that changed since the last [`Self::mark_committed`], or `None` if nothing did.
    pub fn delta(&self, state: &AgentState) -> Option<StepDelta> {
        let snapshot = SessionSnapshot::from_state(state);
        let new_msgs = &state.messages()[self.messages..];
        let new_cps = &state.checkpoints()[self.checkpoints..];
        if new_msgs.is_empty()
            && new_cps.is_empty()
            && self.last_snapshot.as_ref() == Some(&snapshot)
        {
            return None;
        }
        // One step makes at most one LLM call, so its usage belongs to the last new assistant message.
        let usage_owner = (state.usage.llm_calls > self.llm_calls)
            .then(|| {
                new_msgs
                    .iter()
                    .rposition(|m| m.role == oort_llm::Role::Assistant)
            })
            .flatten();
        let messages = new_msgs
            .iter()
            .enumerate()
            .map(|(i, m)| {
                let usage = (Some(i) == usage_owner)
                    .then(|| state.usage.last.clone())
                    .flatten();
                (m.clone(), usage)
            })
            .collect();
        Some(StepDelta {
            first_seq: self.messages as u64,
            messages,
            first_epoch: self.checkpoints as u32 + 1,
            checkpoints: new_cps.to_vec(),
            llm_calls: Vec::new(),
            snapshot,
        })
    }

    pub fn mark_committed(&mut self, state: &AgentState) {
        *self = Self::new(state);
    }
}

/// [`LoopHook`] that commits every step to a [`SessionStore`].
pub struct PersistenceHook {
    store: Arc<dyn SessionStore>,
    session_id: SessionId,
    journal: Mutex<SessionJournal>,
    /// LLM calls observed since the last commit (written with the step that made them).
    calls: Mutex<Vec<LlmCallStats>>,
}

impl PersistenceHook {
    /// `state` must match what is stored (freshly created or loaded).
    pub fn new(store: Arc<dyn SessionStore>, session_id: SessionId, state: &AgentState) -> Self {
        Self {
            store,
            session_id,
            journal: Mutex::new(SessionJournal::new(state)),
            calls: Mutex::default(),
        }
    }
}

#[async_trait]
impl LoopHook for PersistenceHook {
    async fn on_llm_call(&self, stats: &LlmCallStats) {
        self.calls.lock().await.push(stats.clone());
    }

    async fn after_step(&self, state: &AgentState) -> Result<(), AgentError> {
        let mut journal = self.journal.lock().await;
        if let Some(mut delta) = journal.delta(state) {
            let mut calls = self.calls.lock().await;
            delta.llm_calls = calls.clone();
            self.store.commit_step(&self.session_id, &delta).await?;
            calls.clear();
            journal.mark_committed(state);
        }
        Ok(())
    }
}
