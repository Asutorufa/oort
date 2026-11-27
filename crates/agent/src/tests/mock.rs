//! Scripted mocks for the LLM and tool boundaries.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use oort_llm::{ChatRequest, ChatStream, StopReason, StreamEvent, ToolDefinition, Usage};
use serde_json::{Value, json};
use tokio::sync::{Barrier, Notify, mpsc};

use crate::model::ModelClient;
use crate::tool::{LocalTool, LocalToolExecutor, Tool, ToolContext, ToolError, ToolOutput};

/// Pause point inside a scripted stream.
#[derive(Clone)]
pub enum Gate {
    /// Emit `after` events, signal `reached`, then wait for `release` before continuing.
    Pause {
        after: usize,
        reached: Arc<Notify>,
        release: Arc<Notify>,
    },
    /// Emit `after` events, signal `reached`, then never finish.
    Hang { after: usize, reached: Arc<Notify> },
}

pub struct MockTurn {
    pub events: Vec<StreamEvent>,
    pub gate: Option<Gate>,
}

impl MockTurn {
    pub fn text(text: &str, input_tokens: u64) -> Self {
        Self {
            events: vec![
                StreamEvent::TextDelta(text.to_string()),
                usage(input_tokens),
                StreamEvent::Done {
                    stop_reason: Some(StopReason::EndTurn),
                },
            ],
            gate: None,
        }
    }

    /// `calls`: `(id, name, raw_json_args)`.
    pub fn tools(calls: &[(&str, &str, &str)], input_tokens: u64) -> Self {
        let mut events = vec![StreamEvent::TextDelta("Working.".into())];
        for (i, (id, name, args)) in calls.iter().enumerate() {
            events.push(StreamEvent::ToolCallStart {
                index: i,
                id: id.to_string(),
                name: name.to_string(),
            });
            events.push(StreamEvent::ToolCallDelta {
                index: i,
                arguments_delta: args.to_string(),
            });
            events.push(StreamEvent::ToolCallEnd { index: i });
        }
        events.push(usage(input_tokens));
        events.push(StreamEvent::Done {
            stop_reason: Some(StopReason::ToolUse),
        });
        Self { events, gate: None }
    }

    pub fn with_gate(mut self, gate: Gate) -> Self {
        self.gate = Some(gate);
        self
    }
}

fn usage(input_tokens: u64) -> StreamEvent {
    StreamEvent::Usage(Usage {
        input_tokens,
        output_tokens: 10,
        ..Default::default()
    })
}

/// Replays scripted turns in order and records every request.
#[derive(Default)]
pub struct MockModel {
    turns: Mutex<VecDeque<MockTurn>>,
    pub requests: Mutex<Vec<ChatRequest>>,
}

impl MockModel {
    pub fn new(turns: Vec<MockTurn>) -> Arc<Self> {
        Arc::new(Self {
            turns: Mutex::new(turns.into()),
            requests: Mutex::default(),
        })
    }

    pub fn requests(&self) -> Vec<ChatRequest> {
        self.requests.lock().unwrap().clone()
    }
}

#[async_trait]
impl ModelClient for MockModel {
    async fn stream(&self, request: ChatRequest) -> Result<ChatStream, oort_llm::Error> {
        self.requests.lock().unwrap().push(request);
        let turn = self
            .turns
            .lock()
            .unwrap()
            .pop_front()
            .ok_or_else(|| oort_llm::Error::api("mock script exhausted"))?;

        let (tx, rx) = mpsc::channel(64);
        tokio::spawn(async move {
            let split = match &turn.gate {
                Some(Gate::Pause { after, .. } | Gate::Hang { after, .. }) => *after,
                None => turn.events.len(),
            };
            let mut events = turn.events.into_iter();
            for ev in events.by_ref().take(split) {
                if tx.send(Ok(ev)).await.is_err() {
                    return;
                }
            }
            match turn.gate {
                Some(Gate::Pause {
                    reached, release, ..
                }) => {
                    reached.notify_one();
                    release.notified().await;
                }
                Some(Gate::Hang { reached, .. }) => {
                    reached.notify_one();
                    tx.closed().await; // until the consumer drops the stream
                    return;
                }
                None => {}
            }
            for ev in events {
                if tx.send(Ok(ev)).await.is_err() {
                    return;
                }
            }
        });
        let stream = futures_util::stream::unfold(rx, |mut rx| async move {
            rx.recv().await.map(|item| (item, rx))
        });
        Ok(Box::pin(stream))
    }
}

/// Model that always fails (infrastructure error).
pub struct FailingModel;

#[async_trait]
impl ModelClient for FailingModel {
    async fn stream(&self, _request: ChatRequest) -> Result<ChatStream, oort_llm::Error> {
        Err(oort_llm::Error::Api {
            status: Some(401),
            message: "invalid api key".into(),
            body: None,
        })
    }
}

/// Scripted tool behavior.
#[derive(Clone)]
pub enum Behavior {
    Ok(String),
    Fail(String),
    /// Waits on a shared barrier (proves concurrent execution), then succeeds.
    Barrier(Arc<Barrier>),
    /// Signals `started` and waits for cancellation.
    Hang(Arc<Notify>),
}

pub struct MockTool {
    pub name: String,
    pub read_only: bool,
    pub behavior: Behavior,
    pub runs: Arc<AtomicUsize>,
}

impl MockTool {
    pub fn new(name: &str, read_only: bool, behavior: Behavior) -> Arc<Self> {
        Arc::new(Self {
            name: name.into(),
            read_only,
            behavior,
            runs: Arc::default(),
        })
    }

    pub fn runs(&self) -> usize {
        self.runs.load(Ordering::SeqCst)
    }
}

impl Tool for MockTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new(
            &self.name,
            format!("mock tool {}", self.name),
            json!({
                "type": "object",
                "properties": { "path": { "type": "string" }, "content": { "type": "string" } },
            }),
        )
    }

    fn is_read_only(&self) -> bool {
        self.read_only
    }
}

#[async_trait]
impl LocalTool for MockTool {
    async fn run(&self, args: Value, ctx: ToolContext) -> Result<ToolOutput, ToolError> {
        self.runs.fetch_add(1, Ordering::SeqCst);
        match &self.behavior {
            Behavior::Ok(s) => Ok(ToolOutput::ok(format!("{s} {args}"))),
            Behavior::Fail(s) => Err(ToolError::Failed(s.clone())),
            Behavior::Barrier(b) => {
                tokio::time::timeout(Duration::from_secs(2), b.wait())
                    .await
                    .map_err(|_| ToolError::Failed("barrier timeout: not concurrent".into()))?;
                Ok(ToolOutput::ok("joined"))
            }
            Behavior::Hang(started) => {
                started.notify_one();
                ctx.cancel.cancelled().await;
                Err(ToolError::Cancelled)
            }
        }
    }
}

pub fn executor(tools: &[Arc<MockTool>]) -> Arc<LocalToolExecutor> {
    let mut ex = LocalToolExecutor::new();
    for t in tools {
        ex = ex.with_tool(t.clone());
    }
    Arc::new(ex)
}
