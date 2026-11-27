//! The loop: `step()` advances [`Phase`] by one transition; `run_loop()` drives it.
//!
//! ```text
//!            ┌──────── steer / interrupt / hook-appended msgs ───────┐
//!            ▼                                                       │
//!  Idle ──► Ready ──LLM──► (tool calls?) ──yes──► ToolsPending ──hooks──► AwaitingApproval
//!   ▲         │                 │no                    │approved              │ human decision
//!   │         │ interrupt       ▼                      ▼                      │
//!   │         └─► Ready   pending steers? ──yes──► Ready ◄── execute tools ◄──┘
//!   └──────────────────────────no
//! ```

use std::collections::BTreeMap;
use std::sync::Arc;

use futures_util::StreamExt;
use oort_llm::{
    ChatRequest, ChatResponse, ContentPart, Message, Role, StreamEvent, ToolCall, ToolResult,
};
use tokio::sync::mpsc;
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;

use crate::compact::{CompactionInput, Compactor};
use crate::context::{ContextManager, DefaultContextManager};
use crate::error::AgentError;
use crate::hook::{BeforeLlm, HookAction, HookChain, LoopHook};
use crate::io::{AgentEvent, ApprovalDecision, EventSink, Inbox, InterruptPoint, PhaseKind};
use crate::model::ModelClient;
use crate::state::{AgentState, CompactionCheckpoint, Phase, ToolDecision};
use crate::stream::TurnAccumulator;
use crate::tool::{FrozenToolset, INVALID_ARGUMENTS_KEY, ToolContext, ToolExecutor};

/// Model-visible text for calls that never produced a real result because of an interrupt.
pub const INTERRUPTED_TOOL_RESULT: &str =
    "Tool execution was interrupted by the user before it completed. No result is available.";

/// What the caller should do after a [`Agent::step`].
#[derive(Debug, Clone, PartialEq)]
pub enum StepOutcome {
    /// More work is ready; call `step` again.
    Continue,
    /// Paused: these calls need `LoopInput::Approval`. The state can be persisted and resumed.
    NeedsApproval(Vec<ToolCall>),
    /// The model finished; waiting for new user input.
    Completed,
    /// A hook stopped the session.
    Halted(String),
}

/// The agent engine. Stateless across sessions: all session data lives in [`AgentState`].
pub struct Agent {
    model: Arc<dyn ModelClient>,
    executor: Arc<dyn ToolExecutor>,
    tools: FrozenToolset,
    context: Arc<dyn ContextManager>,
    compactor: Option<Arc<dyn Compactor>>,
    hooks: HookChain,
    events: EventSink,
    session_cancel: CancellationToken,
}

/// Builder for [`Agent`].
pub struct AgentBuilder {
    model: Arc<dyn ModelClient>,
    executor: Arc<dyn ToolExecutor>,
    context: Arc<dyn ContextManager>,
    compactor: Option<Arc<dyn Compactor>>,
    hooks: HookChain,
    events: EventSink,
    session_cancel: CancellationToken,
}

impl AgentBuilder {
    pub fn context_manager(mut self, context: Arc<dyn ContextManager>) -> Self {
        self.context = context;
        self
    }

    pub fn compactor(mut self, compactor: Arc<dyn Compactor>) -> Self {
        self.compactor = Some(compactor);
        self
    }

    pub fn hook(mut self, hook: Arc<dyn LoopHook>) -> Self {
        self.hooks.push(hook);
        self
    }

    pub fn events(mut self, tx: mpsc::Sender<AgentEvent>) -> Self {
        self.events = EventSink::new(tx);
        self
    }

    pub fn session_cancel(mut self, token: CancellationToken) -> Self {
        self.session_cancel = token;
        self
    }

    /// Freezes the tool catalog (sorted by name) and builds the agent.
    pub fn build(self) -> Result<Agent, AgentError> {
        Ok(Agent {
            tools: FrozenToolset::freeze(self.executor.as_ref())?,
            model: self.model,
            executor: self.executor,
            context: self.context,
            compactor: self.compactor,
            hooks: self.hooks,
            events: self.events,
            session_cancel: self.session_cancel,
        })
    }
}

impl Agent {
    pub fn builder(model: Arc<dyn ModelClient>, executor: Arc<dyn ToolExecutor>) -> AgentBuilder {
        AgentBuilder {
            model,
            executor,
            context: Arc::new(DefaultContextManager::default()),
            compactor: None,
            hooks: HookChain::default(),
            events: EventSink::none(),
            session_cancel: CancellationToken::new(),
        }
    }

    pub fn session_cancel(&self) -> &CancellationToken {
        &self.session_cancel
    }

    pub fn tools(&self) -> &FrozenToolset {
        &self.tools
    }

    /// The request the next LLM call would send (useful for inspection and tests).
    pub fn preview_request(&self, state: &AgentState) -> ChatRequest {
        self.context.build_request(state, &self.tools)
    }

    /// Creates a session state with this agent's tool catalog frozen into it.
    pub fn new_state(&self, system_prompt: impl Into<String>, initial: Vec<Message>) -> AgentState {
        let mut state = AgentState::new(system_prompt, initial);
        state
            .bind_tools(self.tools.definitions())
            .expect("fresh state accepts any catalog");
        state
    }

    /// Drives `step` for the lifetime of the session. Waits on the inbox while idle or awaiting
    /// approval. Returns `Ok` on `Shutdown`, closed inbox, or halt; `Err` on fatal errors
    /// (the state stays consistent and can be resumed).
    pub async fn run_loop(
        &self,
        state: &mut AgentState,
        inbox: &mut Inbox,
    ) -> Result<(), AgentError> {
        loop {
            let outcome = self.step(state, inbox).await?;
            if inbox.shutdown_requested() {
                return Ok(());
            }
            match outcome {
                StepOutcome::Continue => {}
                StepOutcome::Halted(_) => return Ok(()),
                StepOutcome::Completed | StepOutcome::NeedsApproval(_) => {
                    // Inputs absorbed mid-step still need a step to act on them.
                    if inbox.has_pending()
                        || state.compaction_requested
                        || !state.pending_steers.is_empty()
                    {
                        continue;
                    }
                    let got_input = tokio::select! {
                        _ = self.session_cancel.cancelled() => return Err(AgentError::Cancelled),
                        got = inbox.wait_input() => got,
                    };
                    if !got_input || inbox.shutdown_requested() {
                        return Ok(());
                    }
                }
            }
        }
    }

    /// Advances the state machine by one transition.
    pub async fn step(
        &self,
        state: &mut AgentState,
        inbox: &mut Inbox,
    ) -> Result<StepOutcome, AgentError> {
        state.bind_tools(self.tools.definitions())?;
        if self.session_cancel.is_cancelled() {
            return Err(AgentError::Cancelled);
        }
        // Inputs that arrived between steps are plain buffered inputs: nothing to cancel yet.
        inbox.drain_ready();
        absorb_controls(state, inbox);
        let turn_cancel = self.session_cancel.child_token();
        inbox.begin_turn(turn_cancel.clone());

        let outcome = match state.phase.clone() {
            Phase::Ready => self.step_ready(state, inbox, &turn_cancel).await,
            Phase::ToolsPending { calls } => {
                self.step_tools_pending(state, inbox, calls, &turn_cancel)
                    .await
            }
            Phase::AwaitingApproval { calls, decisions } => {
                self.step_awaiting_approval(state, inbox, calls, decisions, &turn_cancel)
                    .await
            }
            Phase::Idle => self.step_idle(state, inbox, &turn_cancel).await,
            Phase::Halted { reason } => Ok(StepOutcome::Halted(reason)),
        };
        // Whatever happened (including errors), buffered steers/compaction requests are moved
        // into the serializable state so they survive a persist/resume.
        absorb_controls(state, inbox);
        outcome
    }

    // ----- phases -----------------------------------------------------------------------

    async fn step_ready(
        &self,
        state: &mut AgentState,
        inbox: &mut Inbox,
        turn_cancel: &CancellationToken,
    ) -> Result<StepOutcome, AgentError> {
        // Epoch compaction happens only here, before a request is built.
        if self.maybe_compact(state, inbox, turn_cancel).await? {
            self.flush_user_inputs(state, inbox).await;
            return Ok(StepOutcome::Continue);
        }
        self.flush_user_inputs(state, inbox).await;

        let mut ctx = BeforeLlm::new(state);
        let action = self.hooks.before_llm(&mut ctx).await;
        for msg in ctx.into_appended() {
            state.push_message(msg);
        }
        if let HookAction::Halt(reason) = action {
            return Ok(self.halt(state, reason).await);
        }

        state.turn += 1;
        self.events
            .emit(AgentEvent::TurnStarted { turn: state.turn })
            .await;
        let request = self.context.build_request(state, &self.tools);

        let Some(response) = self.stream_turn(request, inbox, turn_cancel).await? else {
            // Interrupted mid-stream: the partial assistant output is discarded (never committed).
            if self.session_cancel.is_cancelled() {
                return Err(AgentError::Cancelled);
            }
            self.events
                .emit(AgentEvent::Interrupted {
                    during: InterruptPoint::LlmStream,
                })
                .await;
            self.flush_user_inputs(state, inbox).await;
            return Ok(StepOutcome::Continue);
        };

        self.commit_response(state, inbox, response).await
    }

    async fn commit_response(
        &self,
        state: &mut AgentState,
        inbox: &mut Inbox,
        response: ChatResponse,
    ) -> Result<StepOutcome, AgentError> {
        state.usage.llm_calls += 1;
        if let Some(usage) = &response.usage {
            state.usage.record(usage);
            self.events
                .emit(AgentEvent::Usage {
                    usage: usage.clone(),
                })
                .await;
        }
        let message = response.clone().into_assistant_message();
        state.push_message(message.clone());
        self.events
            .emit(AgentEvent::AssistantMessage { message })
            .await;

        let calls = response.tool_calls.clone();
        let action = self.hooks.after_llm(state, &response).await;
        absorb_controls(state, inbox);

        if let HookAction::Halt(reason) = action {
            // Keep the protocol valid: every tool call gets a result even when halting.
            for call in &calls {
                let r = ToolResult::error(
                    &call.id,
                    Some(&call.name),
                    format!("Not executed: {reason}"),
                );
                self.commit_result(state, r).await;
            }
            return Ok(self.halt(state, reason).await);
        }
        if !calls.is_empty() {
            self.set_phase(state, Phase::ToolsPending { calls }).await;
            return Ok(StepOutcome::Continue);
        }
        // The model wants to stop, but queued steers keep the loop going.
        if self.flush_user_inputs(state, inbox).await {
            self.set_phase(state, Phase::Ready).await;
            return Ok(StepOutcome::Continue);
        }
        self.set_phase(state, Phase::Idle).await;
        self.events.emit(AgentEvent::Completed).await;
        Ok(StepOutcome::Completed)
    }

    async fn step_tools_pending(
        &self,
        state: &mut AgentState,
        inbox: &mut Inbox,
        calls: Vec<ToolCall>,
        turn_cancel: &CancellationToken,
    ) -> Result<StepOutcome, AgentError> {
        if !inbox.interrupts.is_empty() {
            return Ok(self
                .abort_tools(state, inbox, &calls, InterruptPoint::ToolExecution)
                .await);
        }
        let mut decisions = BTreeMap::new();
        for call in &calls {
            let read_only = self.tools.is_read_only(&call.name);
            let decision = self.hooks.before_tool(state, call, read_only).await;
            decisions.insert(call.id.clone(), decision);
        }
        let pending = unresolved(&calls, &decisions);
        if !pending.is_empty() {
            self.set_phase(state, Phase::AwaitingApproval { calls, decisions })
                .await;
            self.events
                .emit(AgentEvent::ApprovalRequired {
                    calls: pending.clone(),
                })
                .await;
            return Ok(StepOutcome::NeedsApproval(pending));
        }
        self.execute_tools(state, inbox, calls, decisions, turn_cancel)
            .await
    }

    async fn step_awaiting_approval(
        &self,
        state: &mut AgentState,
        inbox: &mut Inbox,
        calls: Vec<ToolCall>,
        mut decisions: BTreeMap<String, ToolDecision>,
        turn_cancel: &CancellationToken,
    ) -> Result<StepOutcome, AgentError> {
        for (call_id, decision) in inbox.approvals.drain(..) {
            if let Some(slot) = decisions.get_mut(&call_id)
                && matches!(slot, ToolDecision::NeedHumanApproval { .. })
            {
                *slot = match decision {
                    ApprovalDecision::Approve => ToolDecision::Approve,
                    ApprovalDecision::Deny { reason } => ToolDecision::Deny { reason },
                };
            }
        }
        if !inbox.interrupts.is_empty() {
            return Ok(self
                .abort_tools(state, inbox, &calls, InterruptPoint::Approval)
                .await);
        }
        let pending = unresolved(&calls, &decisions);
        if !pending.is_empty() {
            state.phase = Phase::AwaitingApproval { calls, decisions };
            return Ok(StepOutcome::NeedsApproval(pending));
        }
        self.execute_tools(state, inbox, calls, decisions, turn_cancel)
            .await
    }

    async fn step_idle(
        &self,
        state: &mut AgentState,
        inbox: &mut Inbox,
        turn_cancel: &CancellationToken,
    ) -> Result<StepOutcome, AgentError> {
        if state.compaction_requested {
            self.maybe_compact(state, inbox, turn_cancel).await?;
        }
        if self.flush_user_inputs(state, inbox).await {
            self.set_phase(state, Phase::Ready).await;
            return Ok(StepOutcome::Continue);
        }
        Ok(StepOutcome::Completed)
    }

    // ----- LLM streaming ----------------------------------------------------------------

    /// Streams one turn. Returns `Ok(None)` if `turn_cancel` fired (partial output discarded).
    async fn stream_turn(
        &self,
        request: ChatRequest,
        inbox: &mut Inbox,
        turn_cancel: &CancellationToken,
    ) -> Result<Option<ChatResponse>, AgentError> {
        let open = self.model.stream(request);
        tokio::pin!(open);
        let mut stream = loop {
            tokio::select! {
                biased;
                _ = turn_cancel.cancelled() => return Ok(None),
                _ = inbox.pump() => {}
                r = &mut open => break r?,
            }
        };

        let mut acc = TurnAccumulator::default();
        loop {
            tokio::select! {
                biased;
                _ = turn_cancel.cancelled() => return Ok(None),
                _ = inbox.pump() => {}
                item = stream.next() => match item {
                    None => break,
                    Some(Err(e)) => return Err(e.into()),
                    Some(Ok(event)) => {
                        match &event {
                            StreamEvent::TextDelta(d) => {
                                self.events.emit(AgentEvent::TextDelta { delta: d.clone() }).await
                            }
                            StreamEvent::ThinkingDelta(d) => {
                                self.events.emit(AgentEvent::ThinkingDelta { delta: d.clone() }).await
                            }
                            _ => {}
                        }
                        acc.push(&event);
                    }
                },
            }
        }
        Ok(Some(acc.finish()))
    }

    // ----- tools -----------------------------------------------------------------------

    /// Runs approved calls (read-only runs concurrently, others sequentially), then commits
    /// exactly one result per call in the original order. Interrupted/unstarted calls get
    /// synthetic error results so every `tool_call_id` is answered before any user message.
    async fn execute_tools(
        &self,
        state: &mut AgentState,
        inbox: &mut Inbox,
        calls: Vec<ToolCall>,
        decisions: BTreeMap<String, ToolDecision>,
        turn_cancel: &CancellationToken,
    ) -> Result<StepOutcome, AgentError> {
        let mut results: Vec<Option<ToolResult>> = vec![None; calls.len()];
        for (i, call) in calls.iter().enumerate() {
            if let Some(ToolDecision::Deny { reason }) = decisions.get(&call.id) {
                results[i] = Some(ToolResult::error(
                    &call.id,
                    Some(&call.name),
                    format!("Tool call denied: {reason}"),
                ));
            } else if let Some(raw) = call.arguments.get(INVALID_ARGUMENTS_KEY) {
                results[i] = Some(ToolResult::error(
                    &call.id,
                    Some(&call.name),
                    format!("Invalid JSON arguments: {raw}. Retry with a valid JSON object."),
                ));
            }
        }

        for group in self.schedule(&calls, &results) {
            if turn_cancel.is_cancelled() {
                break;
            }
            self.run_group(state, inbox, &calls, &group, &mut results, turn_cancel)
                .await;
        }

        let interrupted = turn_cancel.is_cancelled();
        for (call, result) in calls.iter().zip(results) {
            let result = result.unwrap_or_else(|| {
                ToolResult::error(&call.id, Some(&call.name), INTERRUPTED_TOOL_RESULT)
            });
            self.commit_result(state, result).await;
        }
        self.set_phase(state, Phase::Ready).await;
        if self.session_cancel.is_cancelled() {
            return Err(AgentError::Cancelled);
        }
        if interrupted {
            self.events
                .emit(AgentEvent::Interrupted {
                    during: InterruptPoint::ToolExecution,
                })
                .await;
        }
        // Safe checkpoint: all results are in, so interrupts/steers may follow.
        self.flush_user_inputs(state, inbox).await;
        Ok(StepOutcome::Continue)
    }

    /// Groups runnable call indices: consecutive read-only calls share a group.
    fn schedule(&self, calls: &[ToolCall], results: &[Option<ToolResult>]) -> Vec<Vec<usize>> {
        let mut groups: Vec<(bool, Vec<usize>)> = Vec::new();
        for (i, call) in calls.iter().enumerate() {
            if results[i].is_some() {
                continue;
            }
            let ro = self.tools.is_read_only(&call.name);
            match groups.last_mut() {
                Some((true, g)) if ro => g.push(i),
                _ => groups.push((ro, vec![i])),
            }
        }
        groups.into_iter().map(|(_, g)| g).collect()
    }

    async fn run_group(
        &self,
        state: &AgentState,
        inbox: &mut Inbox,
        calls: &[ToolCall],
        group: &[usize],
        results: &mut [Option<ToolResult>],
        turn_cancel: &CancellationToken,
    ) {
        let mut set = JoinSet::new();
        let mut index_of = BTreeMap::new();
        for &i in group {
            let call = calls[i].clone();
            self.events
                .emit(AgentEvent::ToolStarted { call: call.clone() })
                .await;
            let executor = Arc::clone(&self.executor);
            let ctx = ToolContext {
                call_id: call.id.clone(),
                cancel: turn_cancel.child_token(),
            };
            let handle = set.spawn(async move { executor.execute(&call, ctx).await });
            index_of.insert(handle.id(), i);
        }

        while !set.is_empty() {
            let joined = tokio::select! {
                biased;
                _ = turn_cancel.cancelled() => {
                    set.abort_all();
                    // Keep results of tasks that already finished; aborted ones stay `None`.
                    while let Some(j) = set.join_next_with_id().await {
                        if let Ok(j) = j {
                            self.record_result(state, calls, &index_of, results, Ok(j)).await;
                        }
                    }
                    return;
                }
                _ = inbox.pump() => continue,
                Some(j) = set.join_next_with_id() => j,
            };
            self.record_result(state, calls, &index_of, results, joined)
                .await;
        }
    }

    async fn record_result(
        &self,
        state: &AgentState,
        calls: &[ToolCall],
        index_of: &BTreeMap<tokio::task::Id, usize>,
        results: &mut [Option<ToolResult>],
        joined: Result<
            (
                tokio::task::Id,
                Result<crate::tool::ToolOutput, crate::tool::ToolError>,
            ),
            tokio::task::JoinError,
        >,
    ) {
        let (id, outcome) = match joined {
            Ok((id, out)) => (id, Ok(out)),
            Err(e) if e.is_panic() => (e.id(), Err(())),
            Err(_) => return, // cancelled → synthetic result later
        };
        let Some(&i) = index_of.get(&id) else { return };
        let call = &calls[i];
        let mut result = match outcome {
            Ok(Ok(out)) => ToolResult {
                tool_call_id: call.id.clone(),
                name: Some(call.name.clone()),
                content: out.content,
                is_error: out.is_error,
            },
            Ok(Err(err)) => ToolResult::error(&call.id, Some(&call.name), err.to_string()),
            Err(()) => ToolResult::error(
                &call.id,
                Some(&call.name),
                "Tool panicked during execution.",
            ),
        };
        self.hooks.after_tool(state, call, &mut result).await;
        self.events
            .emit(AgentEvent::ToolFinished {
                result: result.clone(),
            })
            .await;
        results[i] = Some(result);
    }

    async fn abort_tools(
        &self,
        state: &mut AgentState,
        inbox: &mut Inbox,
        calls: &[ToolCall],
        during: InterruptPoint,
    ) -> StepOutcome {
        for call in calls {
            let r = ToolResult::error(&call.id, Some(&call.name), INTERRUPTED_TOOL_RESULT);
            self.commit_result(state, r).await;
        }
        self.set_phase(state, Phase::Ready).await;
        self.events.emit(AgentEvent::Interrupted { during }).await;
        self.flush_user_inputs(state, inbox).await;
        StepOutcome::Continue
    }

    async fn commit_result(&self, state: &mut AgentState, result: ToolResult) {
        state.push_message(Message::new(
            Role::Tool,
            vec![ContentPart::ToolResult(result)],
        ));
    }

    // ----- compaction ------------------------------------------------------------------

    /// Runs an epoch compaction if due. Returns `true` if it was interrupted.
    async fn maybe_compact(
        &self,
        state: &mut AgentState,
        inbox: &mut Inbox,
        turn_cancel: &CancellationToken,
    ) -> Result<bool, AgentError> {
        let forced = std::mem::take(&mut state.compaction_requested);
        let Some(compactor) = &self.compactor else {
            if forced {
                tracing::warn!("compaction requested but no compactor configured");
            }
            return Ok(false);
        };
        let Some(up_to_index) = self.context.plan_compaction(state, forced) else {
            return Ok(false);
        };

        let summary = {
            let from = state.latest_checkpoint().map_or(0, |c| c.up_to_index);
            let input = CompactionInput {
                system_prompt: state.system_prompt(),
                previous: state.latest_checkpoint(),
                span: &state.messages()[from..up_to_index],
            };
            let fut = compactor.summarize(input, turn_cancel.clone());
            tokio::pin!(fut);
            loop {
                tokio::select! {
                    biased;
                    _ = turn_cancel.cancelled() => break None,
                    _ = inbox.pump() => {}
                    r = &mut fut => break Some(r),
                }
            }
        };
        let summary_message = match summary {
            Some(Ok(m)) => m,
            Some(Err(AgentError::Cancelled)) | None => {
                if self.session_cancel.is_cancelled() {
                    return Err(AgentError::Cancelled);
                }
                state.compaction_requested = forced; // retry later
                self.events
                    .emit(AgentEvent::Interrupted {
                        during: InterruptPoint::Compaction,
                    })
                    .await;
                return Ok(true);
            }
            Some(Err(e)) => return Err(e),
        };

        state.push_checkpoint(CompactionCheckpoint {
            up_to_index,
            summary_message,
        });
        // The old epoch's usage no longer describes the context; wait for a fresh measurement.
        state.usage.last = None;
        self.events
            .emit(AgentEvent::Compacted {
                up_to_index,
                epoch: state.checkpoints().len(),
            })
            .await;
        Ok(false)
    }

    // ----- helpers ---------------------------------------------------------------------

    /// Appends buffered interrupt messages, then queued steers, to the tail.
    /// Must only be called at a safe checkpoint (no unresolved tool calls).
    async fn flush_user_inputs(&self, state: &mut AgentState, inbox: &mut Inbox) -> bool {
        absorb_controls(state, inbox);
        let interrupts = std::mem::take(&mut inbox.interrupts);
        let had_interrupts = !interrupts.is_empty();
        for m in interrupts {
            state.push_message(m);
        }
        let steers = state.pending_steers.len();
        while let Some(m) = state.pending_steers.pop_front() {
            state.push_message(m);
        }
        if steers > 0 {
            self.events
                .emit(AgentEvent::SteerApplied { count: steers })
                .await;
        }
        had_interrupts || steers > 0
    }

    async fn set_phase(&self, state: &mut AgentState, phase: Phase) {
        let kind = match &phase {
            Phase::Ready => PhaseKind::Ready,
            Phase::ToolsPending { .. } => PhaseKind::ToolsPending,
            Phase::AwaitingApproval { .. } => PhaseKind::AwaitingApproval,
            Phase::Idle => PhaseKind::Idle,
            Phase::Halted { .. } => PhaseKind::Halted,
        };
        state.phase = phase;
        self.events
            .emit(AgentEvent::PhaseChanged { phase: kind })
            .await;
    }

    async fn halt(&self, state: &mut AgentState, reason: String) -> StepOutcome {
        self.set_phase(
            state,
            Phase::Halted {
                reason: reason.clone(),
            },
        )
        .await;
        self.events
            .emit(AgentEvent::Halted {
                reason: reason.clone(),
            })
            .await;
        StepOutcome::Halted(reason)
    }
}

/// Moves buffered steers into the (serialized) state queue and records compaction requests.
fn absorb_controls(state: &mut AgentState, inbox: &mut Inbox) {
    state.pending_steers.extend(inbox.steers.drain(..));
    if std::mem::take(&mut inbox.compact) {
        state.compaction_requested = true;
    }
}

fn unresolved(calls: &[ToolCall], decisions: &BTreeMap<String, ToolDecision>) -> Vec<ToolCall> {
    calls
        .iter()
        .filter(|c| {
            matches!(
                decisions.get(&c.id),
                Some(ToolDecision::NeedHumanApproval { .. })
            )
        })
        .cloned()
        .collect()
}
