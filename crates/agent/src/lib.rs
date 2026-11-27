//! Provider-agnostic LLM agent loop built on `oort-llm`.
//!
//! Layers (all I/O behind `Arc<dyn Trait>` or channels):
//! - [`ModelClient`]: streaming LLM boundary (blanket-implemented for every `oort_llm::LlmClient`).
//! - [`Tool`] / [`ToolExecutor`]: tool schemas vs. the environment that runs them.
//! - [`ContextManager`] / [`Compactor`]: append-only history → request view, epoch compaction.
//! - [`LoopHook`]: middleware (budgets, max turns, approval policy, tracing).
//!
//! The engine ([`Agent::step`] / [`Agent::run_loop`]) is a state machine over a serializable
//! [`AgentState`], fed by [`LoopInput`]s through an [`Inbox`] and reporting [`AgentEvent`]s.
//!
//! # Prefix-cache invariants
//! - History is append-only; dynamic information is appended at the tail as `Role::User`.
//! - Tools are sorted by name and frozen at build time; all JSON uses ordered maps.
//! - No sliding window: context reduction only happens via epoch compaction.

pub mod compact;
pub mod context;
pub mod engine;
pub mod error;
pub mod hook;
pub mod io;
pub mod model;
pub mod state;
mod stream;
pub mod tool;

pub use compact::{
    CompactionInput, CompactionPolicy, Compactor, LlmCompactor, NoopCompactor, RuleBasedCompactor,
    UsageAccounting,
};
pub use context::{ContextManager, DefaultContextManager, RequestParams};
pub use engine::{Agent, AgentBuilder, INTERRUPTED_TOOL_RESULT, StepOutcome};
pub use error::AgentError;
pub use hook::{
    ApprovalPolicy, BeforeLlm, HookAction, HookChain, LoopHook, MaxTurns, TokenBudget, TracingHook,
};
pub use io::{
    AgentEvent, ApprovalDecision, EventSink, Inbox, InterruptPoint, LoopInput, PhaseKind,
};
pub use model::{ModelClient, RetryingModel};
/// Unified IR content block (re-exported from `oort-llm`).
pub use oort_llm::ContentPart as ContentBlock;
pub use state::{AgentState, CompactionCheckpoint, Phase, ToolDecision, UsageLedger};
pub use tool::{
    FrozenToolset, INVALID_ARGUMENTS_KEY, LocalTool, LocalToolExecutor, Tool, ToolContext,
    ToolError, ToolExecutor, ToolOutput,
};

#[cfg(test)]
mod tests;
