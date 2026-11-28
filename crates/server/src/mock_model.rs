//! Offline "mock" provider: a deterministic echo model that streams word by word (so you can
//! interrupt it), calls tools when their names or keywords appear in the message, and reports
//! simulated prefix-cache usage (the previous request's size counts as cached).
//!
//! Keywords: `time`, `list`, `read <path>`, `note <text>`, any exact tool name, `think`, `slow`,
//! `widget:<type>` (sample widget), `skill:<name>` (load_skill).

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

/// A valid example of each widget (unknown kinds are passed through to test validation).
fn sample_widget(kind: &str) -> Value {
    let data = match kind {
        "metrics_chart" => json!({
            "kpis": [{"label": "Requests", "value": "1.2k", "delta": "+8%"}, {"label": "Errors", "value": 3}],
            "series": [{"name": "rps", "points": [{"x": "10:00", "y": 3}, {"x": "10:05", "y": 7}, {"x": "10:10", "y": 5}]}],
            "kind": "area"
        }),
        "data_table" => json!({
            "columns": [{"key": "node", "label": "Node"}, {"key": "latency", "label": "Latency (ms)"}],
            "rows": [{"node": "tokyo-1", "latency": 21}, {"node": "osaka-2", "latency": 35}],
            "row_actions": [{"id": "switch", "label": "Switch to this node"}]
        }),
        "interactive_form" => json!({
            "fields": [
                {"name": "city", "label": "City", "kind": "text", "required": true},
                {"name": "days", "label": "Days", "kind": "number", "default": 2},
                {"name": "plan", "label": "Plan", "kind": "select", "options": ["basic", "pro"]},
                {"name": "notify", "label": "Notify me", "kind": "checkbox"}
            ],
            "submit_label": "Plan the trip"
        }),
        "approval_diff" => json!({
            "summary": "Rename `foo` to `bar` in src/lib.rs",
            "diff": "--- a/src/lib.rs\n+++ b/src/lib.rs\n@@ -1,3 +1,3 @@\n-fn foo() {}\n+fn bar() {}\n fn main() {}\n",
            "files": ["src/lib.rs"]
        }),
        "action_chips" => json!({
            "chips": [{"label": "Summarize", "prompt": "Summarize the conversation"}, {"label": "Compact", "command": "compact"}]
        }),
        _ => json!({}),
    };
    json!({ "widget_type": kind, "title": format!("Sample {kind}"), "data": data })
}

/// A natural reply to a batch of tool results (`msgs` = the assistant message that made the
/// calls, then its tool messages). Widget results are internal notes for the model, so they
/// are acknowledged rather than read out.
fn summarize_results(msgs: &[Message]) -> String {
    let widget_kind = |id: &str| -> Option<String> {
        msgs.iter().flat_map(|m| &m.content).find_map(|p| match p {
            ContentPart::ToolCall(c) if c.id == id && c.name == "render_widget" => c
                .arguments
                .get("widget_type")
                .and_then(Value::as_str)
                .map(str::to_string),
            _ => None,
        })
    };
    let mut lines: Vec<String> = Vec::new();
    let mut waiting = false;
    let mut shown = 0;
    for p in msgs.iter().flat_map(|m| &m.content) {
        let ContentPart::ToolResult(r) = p else {
            continue;
        };
        let name = r.name.as_deref().unwrap_or("tool");
        let snippet: String = r.content.chars().take(240).collect();
        match widget_kind(&r.tool_call_id).as_deref() {
            Some("approval_diff") if r.is_error => {
                lines.push("承知しました。この変更は行いません。".into())
            }
            Some("approval_diff") => {
                lines.push("承認ありがとうございます。変更を反映しました。".into())
            }
            Some(_) if r.is_error => {
                lines.push(format!("カードを表示できませんでした（{snippet}）。"))
            }
            Some(kind) => {
                shown += 1;
                waiting |= matches!(kind, "interactive_form" | "data_table" | "action_chips");
            }
            None if r.is_error => lines.push(format!("⚠️ `{name}` は失敗しました: {snippet}")),
            None => lines.push(format!(
                "**{name}** の結果:\n\n```\n{}\n```",
                snippet.trim_end()
            )),
        }
    }
    if shown > 0 {
        lines.insert(
            0,
            if waiting {
                "上のカードから選んでください。回答をもとに続けます。".into()
            } else {
                "結果をカードにまとめました。".into()
            },
        );
    }
    lines.join("\n\n")
}

/// Keyword-triggered calls of the offline demo (`time`, `list`, `read <path>`, `note <text>`,
/// `widget:<type>`, `skill:<name>`, or a tool's exact name).
fn keyword_calls(
    said: &str,
    lower: &str,
    tools: &[oort_llm::ToolDefinition],
    calls: &mut Vec<(String, Value)>,
) {
    let has_tool = |n: &str| tools.iter().any(|t| t.name == n);
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
    for w in &words {
        if let Some(kind) = w.strip_prefix("widget:")
            && has_tool("render_widget")
        {
            calls.push(("render_widget".into(), sample_widget(kind)));
        }
        if let Some(name) = w.strip_prefix("skill:")
            && has_tool("load_skill")
        {
            calls.push(("load_skill".into(), json!({ "name": name })));
        }
    }
    for t in tools {
        let session_tool = t.name == "render_widget" || t.name == "load_skill";
        if !session_tool && said.contains(&t.name) && !calls.iter().any(|(n, _)| n == &t.name) {
            calls.push((t.name.clone(), json!({ "text": said })));
        }
    }
}

fn explicit_calls(said: &str) -> bool {
    said.lines().any(|l| l.trim().starts_with('@'))
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
        let text = summarize_results(&req.messages[start.saturating_sub(1)..]);
        return Plan {
            thinking: None,
            text,
            calls: vec![],
            slow: false,
        };
    }

    let said = last.map(Message::text).unwrap_or_default();
    if said.trim() == "!params" {
        // Tests: echo the request parameters that are not part of the prefix.
        return Plan {
            thinking: None,
            text: json!({
                "thinking": req.thinking,
                "temperature": req.temperature,
                "max_tokens": req.max_tokens,
                "model": req.model,
            })
            .to_string(),
            calls: vec![],
            slow: false,
        };
    }
    let lower = said.to_lowercase();
    let has_tool = |n: &str| req.tools.iter().any(|t| t.name == n);
    let mut calls = Vec::new();
    if explicit_calls(&said) {
        // `@<tool> {json}` lines call exactly those tools (tests and demos).
        for line in said.lines() {
            if let Some(rest) = line.trim().strip_prefix('@') {
                let (name, args) = rest.split_once(char::is_whitespace).unwrap_or((rest, "{}"));
                if has_tool(name) {
                    let args = serde_json::from_str(args.trim()).unwrap_or_else(|_| json!({}));
                    calls.push((name.to_string(), args));
                }
            }
        }
    } else {
        keyword_calls(&said, &lower, &req.tools, &mut calls);
    }
    let slow = lower.contains("slow");
    let text = if calls.is_empty() {
        let mut t = format!("You said: \"{said}\". This is the offline mock model.");
        if slow {
            t.push_str(&" streaming slowly so you can try Interrupt or Steer".repeat(6));
        }
        t
    } else {
        // Widgets speak for themselves; other tools get a short lead-in.
        let tools = calls.iter().filter(|(n, _)| n != "render_widget").count();
        if tools == 0 {
            String::new()
        } else {
            "確認します。".to_string()
        }
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

/// `!error <status> [once] <message>` in the last user message makes the call fail like a
/// provider would (e.g. `!error 429 once quota exceeded`); `once` fails only the first attempt.
fn simulated_error(req: &ChatRequest) -> Option<oort_llm::Error> {
    static FAILED_ONCE: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());
    let said = req
        .messages
        .iter()
        .rev()
        .find(|m| m.role == Role::User)?
        .text();
    let line = said
        .lines()
        .find_map(|l| l.trim().strip_prefix("!error "))?;
    let (status, rest) = line.split_once(' ').unwrap_or((line, ""));
    let (once, message) = match rest.strip_prefix("once") {
        Some(m) => (true, m.trim()),
        None => (false, rest.trim()),
    };
    if once {
        let mut failed = FAILED_ONCE.lock().unwrap_or_else(|e| e.into_inner());
        if failed.contains(&said) {
            return None;
        }
        failed.push(said.clone());
    }
    Some(oort_llm::Error::Api {
        status: status.parse().ok(),
        message: if message.is_empty() {
            "simulated error".into()
        } else {
            message.into()
        },
        body: None,
    })
}

#[async_trait]
impl ModelClient for MockModel {
    async fn stream(&self, req: ChatRequest) -> Result<ChatStream, oort_llm::Error> {
        if let Some(err) = simulated_error(&req) {
            return Err(err);
        }
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
