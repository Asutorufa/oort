//! Epoch-based compaction.
//!
//! Compaction never deletes history. It appends a [`CompactionCheckpoint`] and the
//! [`crate::ContextManager`] then sends `[System, Tools, summary, messages[up_to_index..]]`.
//! Between compactions the request prefix is byte-stable; a compaction starts a new epoch.

use std::collections::BTreeSet;
use std::sync::Arc;

use async_trait::async_trait;
use oort_llm::{ChatRequest, ContentPart, Message, Role, Usage, collect_stream};
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

use crate::error::AgentError;
use crate::model::ModelClient;
use crate::state::{AgentState, CompactionCheckpoint, tool_call_ids, tool_result_ids};

/// How provider usage maps to "tokens currently in context".
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UsageAccounting {
    /// `input_tokens` already includes cached tokens (OpenAI, Gemini).
    #[default]
    Inclusive,
    /// `input_tokens` excludes cache reads/writes (Anthropic).
    Exclusive,
}

/// When and how much to compact.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CompactionPolicy {
    pub max_context_tokens: u64,
    /// Fire when context usage reaches this fraction of `max_context_tokens` (e.g. `0.8`).
    pub trigger_ratio: f64,
    /// Never summarize the newest `keep_recent_messages` messages.
    pub keep_recent_messages: usize,
    pub accounting: UsageAccounting,
}

impl CompactionPolicy {
    pub fn new(max_context_tokens: u64) -> Self {
        Self {
            max_context_tokens,
            trigger_ratio: 0.8,
            keep_recent_messages: 4,
            accounting: UsageAccounting::default(),
        }
    }

    pub fn context_tokens(&self, usage: &Usage) -> u64 {
        match self.accounting {
            UsageAccounting::Inclusive => usage.input_tokens,
            UsageAccounting::Exclusive => {
                usage.input_tokens
                    + usage.cache_read_tokens.unwrap_or(0)
                    + usage.cache_creation_tokens.unwrap_or(0)
            }
        }
    }

    /// `true` if the last response of the current epoch crossed the threshold.
    pub fn is_over_threshold(&self, state: &AgentState) -> bool {
        let Some(last) = &state.usage.last else {
            return false;
        };
        let threshold = (self.max_context_tokens as f64 * self.trigger_ratio) as u64;
        self.context_tokens(last) >= threshold
    }
}

/// `true` if cutting before `messages[idx]` lands on a complete turn boundary: no tool exchange
/// is in flight (every earlier `ToolCall` has its `ToolResult`) and `messages[idx]` is a user or
/// assistant message, not a tool result. Cutting here never separates a `ToolUse` from its
/// `ToolResult`, and the view `[summary(User), messages[idx..]]` stays protocol-valid.
pub fn is_turn_boundary(messages: &[Message], idx: usize) -> bool {
    let Some(msg) = messages.get(idx) else {
        return false;
    };
    if msg.role == Role::Tool
        || msg.role == Role::System
        || msg
            .content
            .iter()
            .any(|p| matches!(p, ContentPart::ToolResult(_)))
    {
        return false;
    }
    let mut open = BTreeSet::new();
    for m in &messages[..idx] {
        open.extend(tool_call_ids(m));
        for id in tool_result_ids(m) {
            open.remove(id);
        }
    }
    open.is_empty()
}

/// Latest turn boundary `b` with `from < b <= len - keep_recent`.
pub fn select_boundary(messages: &[Message], from: usize, keep_recent: usize) -> Option<usize> {
    let upper = messages.len().saturating_sub(keep_recent);
    (from + 1..=upper)
        .rev()
        .find(|&b| is_turn_boundary(messages, b))
}

/// Input to a [`Compactor`].
pub struct CompactionInput<'a> {
    pub system_prompt: &'a str,
    /// The previous epoch's summary, which the new summary must subsume.
    pub previous: Option<&'a CompactionCheckpoint>,
    /// Raw messages being folded into the new summary.
    pub span: &'a [Message],
}

/// Pluggable summarization strategy.
#[async_trait]
pub trait Compactor: Send + Sync {
    /// Returns the summary message. It is placed right after the system prompt, so it should
    /// be a `Role::User` message (system messages would be merged into the system prompt).
    async fn summarize(
        &self,
        input: CompactionInput<'_>,
        cancel: CancellationToken,
    ) -> Result<Message, AgentError>;
}

/// Compactor that refuses to compact (compaction requests become a no-op error).
pub struct NoopCompactor;

#[async_trait]
impl Compactor for NoopCompactor {
    async fn summarize(
        &self,
        _input: CompactionInput<'_>,
        _cancel: CancellationToken,
    ) -> Result<Message, AgentError> {
        Err(AgentError::Compaction("compaction is disabled".into()))
    }
}

/// Deterministic rule-based compactor: keeps the first `max_chars_per_message` characters of
/// every message's text. Useful for tests and as an offline fallback.
pub struct RuleBasedCompactor {
    pub max_chars_per_message: usize,
}

#[async_trait]
impl Compactor for RuleBasedCompactor {
    async fn summarize(
        &self,
        input: CompactionInput<'_>,
        _cancel: CancellationToken,
    ) -> Result<Message, AgentError> {
        let mut out = String::from("<conversation_summary>\n");
        if let Some(prev) = input.previous {
            out.push_str(&prev.summary_message.text());
            out.push('\n');
        }
        out.push_str(&render_transcript(input.span, self.max_chars_per_message));
        out.push_str("</conversation_summary>");
        Ok(Message::user(out))
    }
}

/// Summarizes with a (typically cheaper/faster) LLM.
pub struct LlmCompactor {
    model: Arc<dyn ModelClient>,
    model_name: Option<String>,
    instructions: String,
}

impl LlmCompactor {
    pub fn new(model: Arc<dyn ModelClient>) -> Self {
        Self {
            model,
            model_name: None,
            instructions: "Summarize the conversation so far for an AI agent that will continue the task. \
                Preserve goals, decisions, constraints, file paths, open questions, and tool findings. \
                Be concise and factual."
                .into(),
        }
    }

    pub fn with_model_name(mut self, name: impl Into<String>) -> Self {
        self.model_name = Some(name.into());
        self
    }

    pub fn with_instructions(mut self, instructions: impl Into<String>) -> Self {
        self.instructions = instructions.into();
        self
    }
}

#[async_trait]
impl Compactor for LlmCompactor {
    async fn summarize(
        &self,
        input: CompactionInput<'_>,
        cancel: CancellationToken,
    ) -> Result<Message, AgentError> {
        let mut transcript = String::new();
        if let Some(prev) = input.previous {
            transcript.push_str("Previous summary:\n");
            transcript.push_str(&prev.summary_message.text());
            transcript.push_str("\n\n");
        }
        transcript.push_str("Conversation:\n");
        transcript.push_str(&render_transcript(input.span, 4_000));

        // Render the span as plain text so the summarizer request carries no tool protocol.
        let mut req =
            ChatRequest::new(vec![Message::user(transcript)]).with_system(&self.instructions);
        req.model = self.model_name.clone();

        let summary = tokio::select! {
            _ = cancel.cancelled() => return Err(AgentError::Cancelled),
            r = async {
                let stream = self.model.stream(req).await?;
                collect_stream(stream).await
            } => r?,
        };
        Ok(Message::user(format!(
            "<conversation_summary>\n{}\n</conversation_summary>",
            summary.content
        )))
    }
}

fn render_transcript(messages: &[Message], max_chars: usize) -> String {
    let mut out = String::new();
    for m in messages {
        for part in &m.content {
            let line = match part {
                ContentPart::Text { text, .. } => format!("[{:?}] {text}", m.role),
                ContentPart::ToolCall(c) => format!("[tool_call {}] {}", c.name, c.arguments),
                ContentPart::ToolResult(r) => {
                    format!(
                        "[tool_result{}] {}",
                        if r.is_error { " error" } else { "" },
                        r.content
                    )
                }
                _ => continue,
            };
            out.extend(line.chars().take(max_chars));
            out.push('\n');
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use oort_llm::ToolCall;
    use serde_json::json;

    #[test]
    fn boundary_never_splits_tool_pairs() {
        let msgs = vec![
            Message::user("a"),
            Message::assistant_parts(vec![ContentPart::ToolCall(ToolCall::new(
                "1",
                "t",
                json!({}),
            ))]),
            Message::tool_result("1", Some("t"), "r", false),
            Message::user("b"),
        ];
        assert!(is_turn_boundary(&msgs, 0));
        assert!(
            is_turn_boundary(&msgs, 1),
            "assistant after resolved history"
        );
        assert!(
            !is_turn_boundary(&msgs, 2),
            "tool result is never a cut point"
        );
        assert!(is_turn_boundary(&msgs, 3));
        assert!(!is_turn_boundary(&msgs, 4), "out of range");
        assert_eq!(select_boundary(&msgs, 0, 0), Some(3));
        assert_eq!(select_boundary(&msgs, 0, 2), Some(1));
        assert_eq!(select_boundary(&msgs, 0, 4), None);
        assert_eq!(select_boundary(&msgs, 3, 0), None);

        // A pending (unresolved) call blocks every later boundary.
        let mut open = msgs.clone();
        open.push(Message::assistant_parts(vec![ContentPart::ToolCall(
            ToolCall::new("2", "t", json!({})),
        )]));
        open.push(Message::user("steer?"));
        assert!(!is_turn_boundary(&open, 5));
    }
}
