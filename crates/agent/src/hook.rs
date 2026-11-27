//! Middleware hook points around LLM calls and tool calls.
//!
//! Hooks can observe state, halt the loop, gate tools, rewrite tool results, and append
//! (never rewrite) messages to the history tail. They cannot touch the cached prefix.

use std::collections::BTreeSet;
use std::sync::Arc;

use async_trait::async_trait;
use oort_llm::{ChatResponse, Message, ToolCall, ToolResult};

use crate::state::{AgentState, ToolDecision};

/// Flow control returned by LLM-level hooks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HookAction {
    Continue,
    Halt(String),
}

/// Context for [`LoopHook::before_llm`].
pub struct BeforeLlm<'a> {
    pub state: &'a AgentState,
    appended: Vec<Message>,
}

impl<'a> BeforeLlm<'a> {
    pub(crate) fn new(state: &'a AgentState) -> Self {
        Self {
            state,
            appended: Vec::new(),
        }
    }

    /// Appends a message to the **tail** of the history before this LLM call
    /// (e.g. current time, environment changes). Prefix-cache safe.
    pub fn append_tail(&mut self, message: Message) {
        self.appended.push(message);
    }

    pub(crate) fn into_appended(self) -> Vec<Message> {
        self.appended
    }
}

/// Middleware hook. Every method has a pass-through default.
#[async_trait]
pub trait LoopHook: Send + Sync {
    async fn before_llm(&self, _ctx: &mut BeforeLlm<'_>) -> HookAction {
        HookAction::Continue
    }

    /// Runs after the assistant message is committed and usage recorded.
    async fn after_llm(&self, _state: &AgentState, _response: &ChatResponse) -> HookAction {
        HookAction::Continue
    }

    async fn before_tool(
        &self,
        _state: &AgentState,
        _call: &ToolCall,
        _read_only: bool,
    ) -> ToolDecision {
        ToolDecision::Approve
    }

    async fn after_tool(&self, _state: &AgentState, _call: &ToolCall, _result: &mut ToolResult) {}
}

/// Ordered list of hooks, combined with "most restrictive wins" semantics.
#[derive(Clone, Default)]
pub struct HookChain {
    hooks: Vec<Arc<dyn LoopHook>>,
}

impl HookChain {
    pub fn push(&mut self, hook: Arc<dyn LoopHook>) {
        self.hooks.push(hook);
    }

    pub(crate) async fn before_llm(&self, ctx: &mut BeforeLlm<'_>) -> HookAction {
        for hook in &self.hooks {
            if let HookAction::Halt(r) = hook.before_llm(ctx).await {
                return HookAction::Halt(r);
            }
        }
        HookAction::Continue
    }

    pub(crate) async fn after_llm(&self, state: &AgentState, resp: &ChatResponse) -> HookAction {
        for hook in &self.hooks {
            if let HookAction::Halt(r) = hook.after_llm(state, resp).await {
                return HookAction::Halt(r);
            }
        }
        HookAction::Continue
    }

    /// `Deny` beats `NeedHumanApproval` beats `Approve`.
    pub(crate) async fn before_tool(
        &self,
        state: &AgentState,
        call: &ToolCall,
        read_only: bool,
    ) -> ToolDecision {
        let mut result = ToolDecision::Approve;
        for hook in &self.hooks {
            match hook.before_tool(state, call, read_only).await {
                d @ ToolDecision::Deny { .. } => return d,
                d @ ToolDecision::NeedHumanApproval { .. } => result = d,
                ToolDecision::Approve => {}
            }
        }
        result
    }

    pub(crate) async fn after_tool(&self, state: &AgentState, call: &ToolCall, r: &mut ToolResult) {
        for hook in &self.hooks {
            hook.after_tool(state, call, r).await;
        }
    }
}

// ---------------------------------------------------------------------------
// Built-in hooks
// ---------------------------------------------------------------------------

/// Halts before starting turn `max_turns + 1`.
pub struct MaxTurns(pub u32);

#[async_trait]
impl LoopHook for MaxTurns {
    async fn before_llm(&self, ctx: &mut BeforeLlm<'_>) -> HookAction {
        if ctx.state.turn >= self.0 {
            HookAction::Halt(format!("max turns ({}) reached", self.0))
        } else {
            HookAction::Continue
        }
    }
}

/// Halts once cumulative input + output tokens exceed the budget.
pub struct TokenBudget {
    pub max_total_tokens: u64,
}

impl TokenBudget {
    fn check(&self, state: &AgentState) -> HookAction {
        let used = state.usage.total_tokens();
        if used >= self.max_total_tokens {
            HookAction::Halt(format!(
                "token budget exhausted ({used}/{})",
                self.max_total_tokens
            ))
        } else {
            HookAction::Continue
        }
    }
}

#[async_trait]
impl LoopHook for TokenBudget {
    async fn before_llm(&self, ctx: &mut BeforeLlm<'_>) -> HookAction {
        self.check(ctx.state)
    }

    async fn after_llm(&self, state: &AgentState, _response: &ChatResponse) -> HookAction {
        self.check(state)
    }
}

/// Static approval policy: deny-list → `Deny`; read-only or allow-list → `Approve`;
/// everything else → `NeedHumanApproval`.
#[derive(Debug, Clone, Default)]
pub struct ApprovalPolicy {
    pub always_allow: BTreeSet<String>,
    pub deny: BTreeSet<String>,
}

#[async_trait]
impl LoopHook for ApprovalPolicy {
    async fn before_tool(&self, _s: &AgentState, call: &ToolCall, read_only: bool) -> ToolDecision {
        if self.deny.contains(&call.name) {
            ToolDecision::Deny {
                reason: format!("tool `{}` is not allowed", call.name),
            }
        } else if read_only || self.always_allow.contains(&call.name) {
            ToolDecision::Approve
        } else {
            ToolDecision::NeedHumanApproval {
                reason: format!("tool `{}` has side effects", call.name),
            }
        }
    }
}

/// Emits `tracing` spans/events for every hook point.
pub struct TracingHook;

#[async_trait]
impl LoopHook for TracingHook {
    async fn before_llm(&self, ctx: &mut BeforeLlm<'_>) -> HookAction {
        tracing::debug!(
            turn = ctx.state.turn,
            messages = ctx.state.messages().len(),
            "llm call"
        );
        HookAction::Continue
    }

    async fn after_llm(&self, state: &AgentState, response: &ChatResponse) -> HookAction {
        tracing::debug!(turn = state.turn, usage = ?response.usage, tool_calls = response.tool_calls.len(), "llm done");
        HookAction::Continue
    }

    async fn after_tool(&self, _s: &AgentState, call: &ToolCall, result: &mut ToolResult) {
        tracing::debug!(tool = %call.name, id = %call.id, is_error = result.is_error, "tool done");
    }
}
