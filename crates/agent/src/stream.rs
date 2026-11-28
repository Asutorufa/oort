//! Accumulates one streamed LLM turn into a `ChatResponse`.

use std::collections::BTreeMap;

use oort_llm::{ChatResponse, StopReason, StreamEvent, ToolCall, Usage};
use serde_json::{Value, json};

use crate::tool::INVALID_ARGUMENTS_KEY;

#[derive(Default)]
struct PendingCall {
    id: String,
    name: String,
    args: String,
    signature: Option<String>,
}

/// Like `oort_llm::StreamCollector`, but tolerant of malformed tool arguments: they are kept
/// as `{ INVALID_ARGUMENTS_KEY: raw }` so the call can be answered with an error result
/// (letting the model self-correct) instead of failing the whole turn.
#[derive(Default)]
pub(crate) struct TurnAccumulator {
    id: Option<String>,
    model: Option<String>,
    text: String,
    thinking: Option<String>,
    signature: Option<String>,
    calls: BTreeMap<usize, PendingCall>,
    usage: Option<Usage>,
    stop_reason: Option<StopReason>,
}

impl TurnAccumulator {
    pub(crate) fn push(&mut self, event: &StreamEvent) {
        match event {
            StreamEvent::MessageStart { id, model } => {
                self.id = id.clone().or(self.id.take());
                self.model = model.clone().or(self.model.take());
            }
            StreamEvent::TextDelta(t) => self.text.push_str(t),
            StreamEvent::ThinkingDelta(t) => {
                self.thinking.get_or_insert_with(String::new).push_str(t)
            }
            StreamEvent::ThinkingSignatureDelta(s) => {
                self.signature.get_or_insert_with(String::new).push_str(s)
            }
            StreamEvent::ToolCallStart { index, id, name } => {
                let c = self.calls.entry(*index).or_default();
                if !id.is_empty() {
                    c.id = id.clone();
                }
                if !name.is_empty() {
                    c.name = name.clone();
                }
            }
            StreamEvent::ToolCallDelta {
                index,
                arguments_delta,
            } => self
                .calls
                .entry(*index)
                .or_default()
                .args
                .push_str(arguments_delta),
            StreamEvent::ToolCallEnd { .. } => {}
            StreamEvent::ToolCallSignature { index, signature } => {
                if !signature.is_empty() {
                    self.calls.entry(*index).or_default().signature = Some(signature.clone());
                }
            }
            StreamEvent::Usage(u) => self.usage = Some(u.clone()),
            StreamEvent::Done { stop_reason } => {
                if stop_reason.is_some() {
                    self.stop_reason = stop_reason.clone();
                }
            }
        }
    }

    pub(crate) fn finish(self) -> ChatResponse {
        let tool_calls = self
            .calls
            .into_values()
            .map(|c| ToolCall::new(c.id, c.name, parse_args(&c.args)).with_signature(c.signature))
            .collect();
        ChatResponse {
            id: self.id,
            model: self.model,
            content: self.text,
            thinking: self.thinking,
            thinking_signature: self.signature,
            tool_calls,
            stop_reason: self.stop_reason,
            usage: self.usage,
        }
    }
}

fn parse_args(raw: &str) -> Value {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return json!({});
    }
    match serde_json::from_str::<Value>(trimmed) {
        Ok(v @ Value::Object(_)) => v,
        _ => json!({ INVALID_ARGUMENTS_KEY: raw }),
    }
}
