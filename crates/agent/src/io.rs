//! Bidirectional channel boundary: [`LoopInput`] in (via [`Inbox`]), [`AgentEvent`] out (via [`EventSink`]).

use crate::metrics::LlmCallStats;
use oort_llm::{Message, ToolCall, ToolResult, Usage};
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

/// Human decision for a call in `Phase::AwaitingApproval`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "decision", rename_all = "snake_case")]
pub enum ApprovalDecision {
    Approve,
    Deny {
        reason: String,
    },
    /// Answers a call waiting for user input (`ToolDecision::NeedUserInput`).
    Answer {
        content: String,
    },
}

/// External input delivered to a running loop.
#[derive(Debug, Clone, PartialEq)]
pub enum LoopInput {
    /// Soft insert: appended at the next safe checkpoint without cancelling anything.
    Steer(Message),
    /// Hard interrupt: cancels the current turn (`turn_cancel`), then appends this message.
    Interrupt(Message),
    /// Resolves a pending approval.
    Approval {
        call_id: String,
        decision: ApprovalDecision,
    },
    /// Requests an epoch compaction before the next LLM call.
    Compact,
    /// Gracefully stops `run_loop` after the current step.
    Shutdown,
}

/// Where a hard interrupt landed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InterruptPoint {
    LlmStream,
    ToolExecution,
    Approval,
    Compaction,
    Idle,
}

/// Lightweight phase label for UIs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PhaseKind {
    Ready,
    ToolsPending,
    AwaitingApproval,
    Idle,
    Halted,
}

/// Output event stream for UIs, loggers, and daemons.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum AgentEvent {
    TurnStarted {
        turn: u32,
    },
    /// Answer text of one LLM call. `step_index` is the call's turn number; `block_id` is
    /// `<message seq>:text`, the position the block will have in history, so deltas of
    /// different steps are never joined.
    TextDelta {
        step_index: u32,
        block_id: String,
        delta: String,
    },
    /// Reasoning of one LLM call (`block_id` is `<message seq>:thinking`).
    ThinkingDelta {
        step_index: u32,
        block_id: String,
        delta: String,
    },
    /// The model moved on from reasoning (to text, a tool call, or the end of the stream).
    ThinkingEnd {
        block_id: String,
        duration_ms: u64,
    },
    /// A provider-hosted server tool invocation observed during streaming.
    ServerToolUse {
        block_id: String,
        id: String,
        name: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        action: Option<String>,
        input: serde_json::Value,
    },
    /// A provider-hosted server tool result observed during streaming.
    ServerToolResult {
        block_id: String,
        tool_use_id: String,
        name: String,
        summary: String,
        is_error: bool,
    },
    /// A citation observed during streaming.
    Citation {
        block_id: String,
        citation: oort_llm::Citation,
    },
    /// A complete assistant message was committed to history.
    AssistantMessage {
        message: Message,
    },
    ToolStarted {
        call: ToolCall,
    },
    ToolFinished {
        result: ToolResult,
    },
    ApprovalRequired {
        calls: Vec<ToolCall>,
    },
    SteerApplied {
        count: usize,
    },
    Interrupted {
        during: InterruptPoint,
    },
    Compacted {
        up_to_index: usize,
        epoch: usize,
    },
    Usage {
        usage: Usage,
    },
    /// An event of a subagent run by this session's tool call `tool_call_id`.
    Subagent {
        tool_call_id: String,
        sub_session_id: String,
        profile: String,
        /// The child's event (named `inner`: `event` is this enum's serde tag).
        inner: Box<AgentEvent>,
    },
    /// The agent's task plan changed (`update_plan`).
    PlanUpdated {
        steps: Vec<PlanStep>,
    },
    /// The session's title was generated (published by the host once it is stored).
    TitleUpdated {
        session_id: String,
        title: String,
    },
    /// Timing and request summary of a completed LLM call.
    LlmCall {
        stats: Box<LlmCallStats>,
    },
    PhaseChanged {
        phase: PhaseKind,
    },
    Completed,
    Halted {
        reason: String,
    },
}

/// One step of an agent's task plan.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanStep {
    pub id: String,
    pub title: String,
    pub status: PlanStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlanStatus {
    Pending,
    InProgress,
    Done,
    Skipped,
}

/// Event sender. Sending never fails the loop: a detached UI must not kill the agent.
#[derive(Debug, Clone, Default)]
pub struct EventSink {
    tx: Option<mpsc::Sender<AgentEvent>>,
}

impl EventSink {
    pub fn new(tx: mpsc::Sender<AgentEvent>) -> Self {
        Self { tx: Some(tx) }
    }

    /// Discards all events.
    pub fn none() -> Self {
        Self { tx: None }
    }

    /// Sends an event (with back-pressure); a closed receiver is ignored.
    pub async fn emit(&self, event: AgentEvent) {
        if let Some(tx) = &self.tx {
            let _ = tx.send(event).await;
        }
    }
}

/// Receiving side of [`LoopInput`]s, plus a buffer of inputs that arrived mid-step.
///
/// Inputs are absorbed into the buffer as they arrive; the engine applies them at safe
/// checkpoints. `Interrupt` additionally cancels the current turn token immediately.
#[derive(Debug, Default)]
pub struct Inbox {
    rx: Option<mpsc::Receiver<LoopInput>>,
    turn_cancel: Option<CancellationToken>,
    pub(crate) steers: Vec<Message>,
    pub(crate) interrupts: Vec<Message>,
    pub(crate) approvals: Vec<(String, ApprovalDecision)>,
    pub(crate) compact: bool,
    shutdown: bool,
}

impl Inbox {
    pub fn new(rx: mpsc::Receiver<LoopInput>) -> Self {
        Self {
            rx: Some(rx),
            ..Default::default()
        }
    }

    /// Creates a connected `(Sender, Inbox)` pair.
    pub fn channel(capacity: usize) -> (mpsc::Sender<LoopInput>, Self) {
        let (tx, rx) = mpsc::channel(capacity);
        (tx, Self::new(rx))
    }

    /// An inbox with no sender (for driving `step` manually).
    pub fn detached() -> Self {
        Self::default()
    }

    /// `true` once every sender is dropped.
    pub fn is_closed(&self) -> bool {
        self.rx.is_none()
    }

    pub fn shutdown_requested(&self) -> bool {
        self.shutdown
    }

    /// `true` if inputs are buffered but not yet applied by the engine.
    pub fn has_pending(&self) -> bool {
        !self.steers.is_empty()
            || !self.interrupts.is_empty()
            || !self.approvals.is_empty()
            || self.compact
    }

    pub(crate) fn begin_turn(&mut self, turn_cancel: CancellationToken) {
        self.turn_cancel = Some(turn_cancel);
    }

    fn absorb(&mut self, input: LoopInput) {
        match input {
            LoopInput::Steer(m) => self.steers.push(m),
            LoopInput::Interrupt(m) => {
                self.interrupts.push(m);
                if let Some(token) = &self.turn_cancel {
                    token.cancel();
                }
            }
            LoopInput::Approval { call_id, decision } => self.approvals.push((call_id, decision)),
            LoopInput::Compact => self.compact = true,
            LoopInput::Shutdown => self.shutdown = true,
        }
    }

    /// Absorbs every input that is already queued, without waiting.
    pub(crate) fn drain_ready(&mut self) {
        while let Some(rx) = &mut self.rx {
            match rx.try_recv() {
                Ok(input) => self.absorb(input),
                Err(mpsc::error::TryRecvError::Empty) => break,
                Err(mpsc::error::TryRecvError::Disconnected) => self.rx = None,
            }
        }
    }

    /// Waits for one input and absorbs it. Never resolves once closed, so it is safe
    /// to use as a `select!` branch alongside real work.
    pub(crate) async fn pump(&mut self) {
        if !self.wait_input().await {
            std::future::pending::<()>().await;
        }
    }

    /// Waits for one input and absorbs it; returns `false` if the channel is closed.
    pub(crate) async fn wait_input(&mut self) -> bool {
        let Some(rx) = &mut self.rx else {
            return false;
        };
        match rx.recv().await {
            Some(input) => {
                self.absorb(input);
                true
            }
            None => {
                self.rx = None;
                false
            }
        }
    }
}
