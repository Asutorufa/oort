//! Offline "mock" provider: a deterministic echo model that streams word by word (so you can
//! interrupt it), calls tools when their names or keywords appear in the message, and reports
//! simulated prefix-cache usage (the previous request's size counts as cached).
//!
//! Keywords: `time`, `list`, `read <path>`, `note <text>`, any exact tool name, `think`, `slow`.

use std::time::Duration;

use async_trait::async_trait;
use oort_agent::ModelClient;
use oort_llm::{
    ChatRequest, ChatStream, ContentPart, Message, Role, StopReason, StreamEvent, Usage,
};
use serde_json::{Value, json};
use tokio::sync::mpsc;

pub struct MockModel {
    pub word_delay: Duration,
}

impl Default for MockModel {
    fn default() -> Self {
        Self {
            word_delay: Duration::from_millis(30),
        }
    }
}

struct Plan {
    thinking: Option<String>,
    text: String,
    calls: Vec<(String, Value)>,
    slow: bool,
}

fn estimate_tokens(v: &impl serde::Serialize) -> u64 {
    (serde_json::to_string(v).map(|s| s.len()).unwrap_or(0) as u64).div_ceil(4)
}

fn plan(req: &ChatRequest) -> Plan {
    let last = req.messages.last();
    if last.is_some_and(|m| m.role == Role::Tool) {
        // Answer every tool result of the trailing tool batch.
        let start = req
            .messages
            .iter()
            .rposition(|m| m.role != Role::Tool)
            .map_or(0, |i| i + 1);
        let mut text = String::from("Tool results:\n");
        for msg in &req.messages[start..] {
            for p in &msg.content {
                if let ContentPart::ToolResult(r) = p {
                    let snippet: String = r.content.chars().take(200).collect();
                    let flag = if r.is_error { " (error)" } else { "" };
                    text.push_str(&format!(
                        "- {}{flag}: {snippet}\n",
                        r.name.as_deref().unwrap_or("?")
                    ));
                }
            }
        }
        return Plan {
            thinking: None,
            text,
            calls: vec![],
            slow: false,
        };
    }

    let said = last.map(Message::text).unwrap_or_default();
    let lower = said.to_lowercase();
    let has_tool = |n: &str| req.tools.iter().any(|t| t.name == n);
    let mut calls = Vec::new();
    if lower.contains("time") && has_tool("get_time") {
        calls.push(("get_time".to_string(), json!({})));
    }
    if lower.contains("list") && has_tool("list_dir") {
        calls.push(("list_dir".into(), json!({ "path": "." })));
    }
    let words: Vec<&str> = said.split_whitespace().collect();
    if let Some(i) = words.iter().position(|w| w.eq_ignore_ascii_case("read"))
        && let Some(path) = words.get(i + 1)
        && has_tool("read_file")
    {
        calls.push(("read_file".into(), json!({ "path": path })));
    }
    if let Some(i) = lower.find("note ").filter(|_| has_tool("write_note")) {
        calls.push(("write_note".into(), json!({ "text": said[i + 5..].trim() })));
    }
    for t in &req.tools {
        if said.contains(&t.name) && !calls.iter().any(|(n, _)| n == &t.name) {
            calls.push((t.name.clone(), json!({ "text": said })));
        }
    }
    let slow = lower.contains("slow");
    let text = if calls.is_empty() {
        let mut t = format!("You said: \"{said}\". This is the offline mock model.");
        if slow {
            t.push_str(&" streaming slowly so you can try Interrupt or Steer".repeat(6));
        }
        t
    } else {
        format!("Calling {} tool(s).", calls.len())
    };
    Plan {
        thinking: lower
            .contains("think")
            .then(|| format!("The user said {said:?}. I should answer briefly.")),
        text,
        calls,
        slow,
    }
}

#[async_trait]
impl ModelClient for MockModel {
    async fn stream(&self, req: ChatRequest) -> Result<ChatStream, oort_llm::Error> {
        let plan = plan(&req);
        let total = estimate_tokens(&(&req.system, &req.tools, &req.messages));
        let prev_prefix = &req.messages[..req.messages.len().saturating_sub(1)];
        let cached = if prev_prefix.is_empty() {
            0
        } else {
            estimate_tokens(&(&req.system, &req.tools, prev_prefix))
        };
        let usage = Usage {
            input_tokens: total,
            output_tokens: estimate_tokens(&plan.text),
            cache_read_tokens: Some(cached.min(total)),
            ..Default::default()
        };
        let delay = if plan.slow {
            self.word_delay * 4
        } else {
            self.word_delay
        };
        let turn = req.messages.len();

        let (tx, mut rx) = mpsc::channel(64);
        tokio::spawn(async move {
            let send = |ev| tx.send(Ok(ev));
            if send(StreamEvent::MessageStart {
                id: Some(format!("mock-{turn}")),
                model: Some("mock".into()),
            })
            .await
            .is_err()
            {
                return;
            }
            if let Some(t) = plan.thinking {
                for w in t.split_inclusive(' ') {
                    tokio::time::sleep(delay).await;
                    if send(StreamEvent::ThinkingDelta(w.into())).await.is_err() {
                        return;
                    }
                }
            }
            for w in plan.text.split_inclusive(' ') {
                tokio::time::sleep(delay).await;
                if send(StreamEvent::TextDelta(w.into())).await.is_err() {
                    return;
                }
            }
            let has_calls = !plan.calls.is_empty();
            for (i, (name, args)) in plan.calls.into_iter().enumerate() {
                let events = [
                    StreamEvent::ToolCallStart {
                        index: i,
                        id: format!("mock_{turn}_{i}"),
                        name,
                    },
                    StreamEvent::ToolCallDelta {
                        index: i,
                        arguments_delta: args.to_string(),
                    },
                    StreamEvent::ToolCallEnd { index: i },
                ];
                for ev in events {
                    if send(ev).await.is_err() {
                        return;
                    }
                }
            }
            let stop = if has_calls {
                StopReason::ToolUse
            } else {
                StopReason::EndTurn
            };
            let _ = send(StreamEvent::Usage(usage)).await;
            let _ = send(StreamEvent::Done {
                stop_reason: Some(stop),
            })
            .await;
        });
        Ok(Box::pin(futures_util::stream::poll_fn(move |cx| {
            rx.poll_recv(cx)
        })))
    }
}
