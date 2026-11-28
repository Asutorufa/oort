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
pub mod composite;
pub mod config_store;
pub mod context;
pub mod engine;
pub mod error;
pub mod hook;
pub mod io;
pub mod metrics;
pub mod model;
pub mod secret;
pub mod skill_inject;
pub mod state;
pub mod store;
mod stream;
#[cfg(any(test, feature = "testing"))]
pub mod testing;
pub mod tool;

pub use compact::{
    CompactionInput, CompactionPolicy, Compactor, LlmCompactor, NoopCompactor, RuleBasedCompactor,
    UsageAccounting,
};
pub use composite::{CompositeBuilder, CompositeToolExecutor, Namespace, PinnedToolExecutor};
pub use config_store::{
    ApprovalPolicyKind, ConfigStore, McpAuth, McpServerRecord, McpTransportKind, MemoryHit,
    MemoryQuery, MemoryRecord, MemoryStore, OAuthClientSource, OAuthStatus, OAuthTokenRecord,
    OAuthTokenStore, PresetRecord, ProviderKind, ProviderRecord, SkillExecutionMode,
    SkillInvocation, SkillRecord, SkillStore,
};
pub use context::{ContextManager, DefaultContextManager, RequestParams};
pub use engine::{Agent, AgentBuilder, INTERRUPTED_TOOL_RESULT, StepOutcome};
pub use error::AgentError;
pub use hook::{
    ApprovalPolicy, BeforeLlm, HookAction, HookChain, LoopHook, MaxTurns, TokenBudget, TracingHook,
};
pub use io::{
    AgentEvent, ApprovalDecision, EventSink, Inbox, InterruptPoint, LoopInput, PhaseKind,
    PlanStatus, PlanStep,
};
pub use metrics::{LlmCallStats, RequestHashes, RequestSummary};
pub use model::{ModelClient, RetryingModel};
/// Unified IR content block (re-exported from `oort-llm`).
pub use oort_llm::ContentPart as ContentBlock;
pub use state::{AgentState, CompactionCheckpoint, Phase, StateParts, ToolDecision, UsageLedger};
pub use store::{
    Lineage, NewSession, PendingCalls, PersistenceHook, SessionId, SessionJournal, SessionMeta,
    SessionSnapshot, SessionStatus, SessionStore, StepDelta, StoreError, StoredSession,
};
pub use tool::{
    FrozenToolset, INVALID_ARGUMENTS_KEY, LocalTool, LocalToolExecutor, Tool, ToolContext,
    ToolError, ToolExecutor, ToolOutput,
};

#[cfg(test)]
mod tests;
