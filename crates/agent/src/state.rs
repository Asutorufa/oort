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
