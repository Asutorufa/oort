use thiserror::Error;

/// Fatal, infrastructure-level errors that abort a [`crate::Agent::step`].
///
/// Tool failures (bad arguments, missing files, non-zero exit codes, …) are **never**
/// represented here: they become `ToolResult { is_error: true }` so the model can self-correct.
#[derive(Debug, Error)]
pub enum AgentError {
    /// Network, authentication, or protocol failure from the LLM provider.
    #[error("LLM error: {0}")]
    Llm(#[from] oort_llm::Error),

    /// The whole session was cancelled via `session_cancel`.
    #[error("session cancelled")]
    Cancelled,

    /// The compactor failed to produce a summary.
    #[error("compaction failed: {0}")]
    Compaction(String),

    /// The tool catalog differs from the one frozen into the [`crate::AgentState`]
    /// (resuming with different tools would invalidate the prefix cache).
    #[error("tool catalog mismatch: {0}")]
    ToolCatalogMismatch(String),

    /// Invalid configuration (e.g. duplicate tool names).
    #[error("invalid configuration: {0}")]
    InvalidConfig(String),
}
