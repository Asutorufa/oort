//! Serializable agent state: the single source of truth for a session.

use std::collections::{BTreeMap, VecDeque};

use oort_llm::{ContentPart, Message, Role, ToolCall, ToolDefinition, Usage};
use serde::{Deserialize, Serialize};

use crate::error::AgentError;

/// Hook / human decision for a single tool call.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "decision", rename_all = "snake_case")]
pub enum ToolDecision {
    Approve,
    Deny { reason: String },
    NeedHumanApproval { reason: String },
}

/// Where the state machine currently is. Each [`crate::Agent::step`] advances it by one transition.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "phase", rename_all = "snake_case")]
pub enum Phase {
    /// The next step calls the LLM. Invariant: history has no unresolved tool calls.
    Ready,
    /// The last assistant message requested these tools; hooks have not run yet.
    ToolsPending { calls: Vec<ToolCall> },
    /// Some calls need a human decision. `decisions` is keyed by `tool_call_id`.
    AwaitingApproval {
        calls: Vec<ToolCall>,
        decisions: BTreeMap<String, ToolDecision>,
    },
    /// The model finished its turn; waiting for new input.
    Idle,
    /// Stopped by a hook (budget, max turns, …). Terminal.
    Halted { reason: String },
}

/// Result of an epoch compaction: `messages[..up_to_index]` are represented by `summary_message`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CompactionCheckpoint {
    pub up_to_index: usize,
    pub summary_message: Message,
}

/// Cumulative token accounting.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct UsageLedger {
    pub total_input_tokens: u64,
    pub total_output_tokens: u64,
    pub llm_calls: u64,
    /// Usage of the most recent LLM response in the current epoch (`None` right after compaction).
    pub last: Option<Usage>,
}

impl UsageLedger {
    pub fn total_tokens(&self) -> u64 {
        self.total_input_tokens + self.total_output_tokens
    }

    pub(crate) fn record(&mut self, usage: &Usage) {
        self.total_input_tokens += usage.input_tokens
            + usage.cache_read_tokens.unwrap_or(0)
            + usage.cache_creation_tokens.unwrap_or(0);
        self.total_output_tokens += usage.output_tokens;
        self.last = Some(usage.clone());
    }
}

/// Complete, serializable session state.
///
/// # Invariants
/// - `messages` and `checkpoints` are **append-only**: only `push_*` mutate them, never in place.
///   This keeps the raw history auditable and the request prefix byte-stable.
/// - `system_prompt` and `tool_snapshot` are frozen at session start.
/// - Whenever `phase` is `Ready` or `Idle`, every `ToolCall` in `messages` has a matching `ToolResult`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentState {
    system_prompt: String,
    tool_snapshot: Option<Vec<ToolDefinition>>,
    messages: Vec<Message>,
    checkpoints: Vec<CompactionCheckpoint>,
    pub phase: Phase,
    /// Soft-inserted user messages waiting for the next safe checkpoint.
    pub pending_steers: VecDeque<Message>,
    /// Manual compaction request (from `LoopInput::Compact`).
    pub compaction_requested: bool,
    /// Number of LLM turns started.
    pub turn: u32,
    pub usage: UsageLedger,
}

impl AgentState {
    /// Creates a new session. If `initial` is non-empty the agent starts `Ready`, otherwise `Idle`.
    pub fn new(system_prompt: impl Into<String>, initial: Vec<Message>) -> Self {
        let phase = if initial.is_empty() {
            Phase::Idle
        } else {
            Phase::Ready
        };
        Self {
            system_prompt: system_prompt.into(),
            tool_snapshot: None,
            messages: initial,
            checkpoints: Vec::new(),
            phase,
            pending_steers: VecDeque::new(),
            compaction_requested: false,
            turn: 0,
            usage: UsageLedger::default(),
        }
    }

    /// Like [`Self::new`], with the tool catalog frozen up front. Useful when the state must be
    /// stored (to obtain a session id) before the agent that persists it is built.
    pub fn with_tools(
        system_prompt: impl Into<String>,
        initial: Vec<Message>,
        tools: &crate::tool::FrozenToolset,
    ) -> Self {
        let mut state = Self::new(system_prompt, initial);
        state.tool_snapshot = Some(tools.definitions().to_vec());
        state
    }

    pub fn system_prompt(&self) -> &str {
        &self.system_prompt
    }

    /// Raw, never-compacted history.
    pub fn messages(&self) -> &[Message] {
        &self.messages
    }

    pub fn checkpoints(&self) -> &[CompactionCheckpoint] {
        &self.checkpoints
    }

    pub fn latest_checkpoint(&self) -> Option<&CompactionCheckpoint> {
        self.checkpoints.last()
    }

    /// The messages the LLM sees: `[latest_summary?, messages[up_to_index..]]`.
    pub fn active_view(&self) -> Vec<Message> {
        let (summary, start) = match self.latest_checkpoint() {
            Some(cp) => (Some(cp.summary_message.clone()), cp.up_to_index),
            None => (None, 0),
        };
        let mut view: Vec<Message> = summary.into_iter().collect();
        view.extend_from_slice(&self.messages[start..]);
        view
    }

    pub fn tool_snapshot(&self) -> Option<&[ToolDefinition]> {
        self.tool_snapshot.as_deref()
    }

    /// Appends to the tail of the history. `Role::System` is rewritten to `Role::User`
    /// because system messages are merged into the system prompt by provider adapters,
    /// which would mutate the cached prefix.
    pub(crate) fn push_message(&mut self, mut message: Message) {
        if message.role == Role::System {
            message.role = Role::User;
        }
        self.messages.push(message);
    }

    pub(crate) fn push_checkpoint(&mut self, checkpoint: CompactionCheckpoint) {
        let prev = self.latest_checkpoint().map_or(0, |c| c.up_to_index);
        assert!(
            checkpoint.up_to_index > prev && checkpoint.up_to_index <= self.messages.len(),
            "compaction checkpoints must advance monotonically within the history"
        );
        debug_assert!(crate::compact::is_turn_boundary(
            &self.messages,
            checkpoint.up_to_index
        ));
        self.checkpoints.push(checkpoint);
    }

    /// Decomposes the state into its parts (for storage backends).
    pub fn into_parts(self) -> StateParts {
        StateParts {
            system_prompt: self.system_prompt,
            tool_snapshot: self.tool_snapshot,
            messages: self.messages,
            checkpoints: self.checkpoints,
            phase: self.phase,
            pending_steers: self.pending_steers,
            compaction_requested: self.compaction_requested,
            turn: self.turn,
            usage: self.usage,
        }
    }

    /// Rebuilds a state from stored parts, re-checking every invariant that `push_*` normally
    /// guarantees: no system messages in history, monotonic checkpoints on turn boundaries,
    /// and no unresolved tool calls while `Ready`/`Idle`.
    pub fn from_parts(parts: StateParts) -> Result<Self, AgentError> {
        let invalid = |m: String| Err(AgentError::InvalidState(m));
        if let Some(i) = parts.messages.iter().position(|m| m.role == Role::System) {
            return invalid(format!("message {i} has role system"));
        }
        let mut prev = 0;
        for (epoch, cp) in parts.checkpoints.iter().enumerate() {
            if cp.up_to_index <= prev || cp.up_to_index > parts.messages.len() {
                return invalid(format!("checkpoint {epoch} is not monotonic / in range"));
            }
            if !crate::compact::is_turn_boundary(&parts.messages, cp.up_to_index) {
                return invalid(format!("checkpoint {epoch} splits a tool exchange"));
            }
            prev = cp.up_to_index;
        }
        if matches!(parts.phase, Phase::Ready | Phase::Idle) && has_open_calls(&parts.messages) {
            return invalid("history has unresolved tool calls".into());
        }
        Ok(Self {
            system_prompt: parts.system_prompt,
            tool_snapshot: parts.tool_snapshot,
            messages: parts.messages,
            checkpoints: parts.checkpoints,
            phase: parts.phase,
            pending_steers: parts.pending_steers,
            compaction_requested: parts.compaction_requested,
            turn: parts.turn,
            usage: parts.usage,
        })
    }

    /// Freezes the tool catalog on first use; afterwards verifies it is unchanged.
    pub(crate) fn bind_tools(&mut self, defs: &[ToolDefinition]) -> Result<(), AgentError> {
        match &self.tool_snapshot {
            None => {
                self.tool_snapshot = Some(defs.to_vec());
                Ok(())
            }
            Some(frozen) if frozen.as_slice() == defs => Ok(()),
            Some(frozen) => Err(AgentError::ToolCatalogMismatch(format!(
                "state has {:?}, executor has {:?}",
                frozen.iter().map(|t| &t.name).collect::<Vec<_>>(),
                defs.iter().map(|t| &t.name).collect::<Vec<_>>()
            ))),
        }
    }
}

/// All fields of an [`AgentState`], public for storage backends.
/// Rebuild with [`AgentState::from_parts`], which validates the invariants.
#[derive(Debug, Clone, PartialEq)]
pub struct StateParts {
    pub system_prompt: String,
    pub tool_snapshot: Option<Vec<ToolDefinition>>,
    pub messages: Vec<Message>,
    pub checkpoints: Vec<CompactionCheckpoint>,
    pub phase: Phase,
    pub pending_steers: VecDeque<Message>,
    pub compaction_requested: bool,
    pub turn: u32,
    pub usage: UsageLedger,
}

fn has_open_calls(messages: &[Message]) -> bool {
    let mut open = std::collections::BTreeSet::new();
    for m in messages {
        open.extend(tool_call_ids(m));
        for id in tool_result_ids(m) {
            open.remove(id);
        }
    }
    !open.is_empty()
}

/// Collects tool call ids from a message.
pub(crate) fn tool_call_ids(message: &Message) -> impl Iterator<Item = &str> {
    message.content.iter().filter_map(|p| match p {
        ContentPart::ToolCall(c) => Some(c.id.as_str()),
        _ => None,
    })
}

/// Collects tool result ids from a message.
pub(crate) fn tool_result_ids(message: &Message) -> impl Iterator<Item = &str> {
    message.content.iter().filter_map(|p| match p {
        ContentPart::ToolResult(r) => Some(r.tool_call_id.as_str()),
        _ => None,
    })
}
