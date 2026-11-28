//! Records what model and tool calls cost, by wrapping the model client and the tool executor.
//!
//! Both wrappers only observe: requests, events, tool catalogs and results pass through
//! unchanged, so the conversation history and the provider's prefix cache are unaffected.
//! A failed ledger write is logged and never fails the turn.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Instant;

use async_trait::async_trait;
use futures_util::StreamExt;
use futures_util::stream::unfold;
use oort_agent::{
    ModelClient, ProviderKind, Tool, ToolContext, ToolError, ToolExecutor, ToolOutput,
};
use oort_llm::{ChatRequest, ChatStream, StreamEvent, ToolCall, Usage};
use oort_storage::analytics::{
    AnalyticsStore, CallPurpose, LlmUsageRecord, ToolKind, ToolUsageRecord,
};
use oort_storage::pricing::TokenCounts;
use tokio::task::JoinHandle;

/// Who the calls of one client belong to.
#[derive(Clone)]
pub struct Meter {
    pub store: Arc<dyn AnalyticsStore>,
    pub session_id: Option<String>,
    pub provider_id: Option<String>,
    pub vendor: String,
    pub kind: ProviderKind,
    /// The model of requests that do not name one (the client's default).
    pub model: String,
}

pub struct MeteredModel {
    inner: Arc<dyn ModelClient>,
    meter: Meter,
    purpose: CallPurpose,
}

impl MeteredModel {
    pub fn wrap(
        inner: Arc<dyn ModelClient>,
        meter: Meter,
        purpose: CallPurpose,
    ) -> Arc<dyn ModelClient> {
        Arc::new(Self {
            inner,
            meter,
            purpose,
        })
    }
}

#[async_trait]
impl ModelClient for MeteredModel {
    async fn stream(&self, request: ChatRequest) -> Result<ChatStream, oort_llm::Error> {
        let model = request
            .model
            .clone()
            .unwrap_or_else(|| self.meter.model.clone());
        let started = Instant::now();
        let inner = self.inner.stream(request).await?;
        let call = Call {
            meter: self.meter.clone(),
            purpose: self.purpose,
            model,
            started,
            ttft_ms: None,
            usage: None,
            server_tools: BTreeMap::new(),
            done: false,
        };
        Ok(Box::pin(unfold(Some((inner, call)), |state| async move {
            let (mut inner, mut call) = state?;
            match inner.next().await {
                Some(item) => {
                    if let Ok(event) = &item {
                        call.observe(event);
                    }
                    Some((item, Some((inner, call))))
                }
                None => {
                    // Awaited so the row exists once the turn is over; the task itself
                    // survives an interrupt that drops this stream.
                    if let Some(written) = call.finish() {
                        let _ = written.await;
                    }
                    None
                }
            }
        })))
    }
}

/// One call in flight. Recorded when the stream ends, or when it is dropped (interrupt, error).
struct Call {
    meter: Meter,
    purpose: CallPurpose,
    model: String,
    started: Instant,
    ttft_ms: Option<u64>,
    usage: Option<Usage>,
    /// Provider-hosted tool calls still waiting for their result.
    server_tools: BTreeMap<String, (String, Instant)>,
    done: bool,
}

impl Call {
    fn observe(&mut self, event: &StreamEvent) {
        if self.ttft_ms.is_none()
            && matches!(
                event,
                StreamEvent::TextDelta(_)
                    | StreamEvent::ThinkingDelta(_)
                    | StreamEvent::ToolCallStart { .. }
                    | StreamEvent::ServerToolUse { .. }
            )
        {
            self.ttft_ms = Some(self.started.elapsed().as_millis() as u64);
        }
        match event {
            StreamEvent::Usage(u) => self.usage = Some(u.clone()),
            StreamEvent::ServerToolUse { id, name, .. } => {
                self.server_tools
                    .entry(id.clone())
                    .or_insert_with(|| (name.clone(), Instant::now()));
            }
            StreamEvent::ServerToolResult {
                tool_use_id,
                name,
                is_error,
                ..
            } => {
                let (name, since) = match self.server_tools.remove(tool_use_id) {
                    Some((name, since)) => (name, Some(since)),
                    None => (name.clone(), None),
                };
                self.record_server_tool(name, since, *is_error);
            }
            _ => {}
        }
    }

    fn record_server_tool(&self, name: String, since: Option<Instant>, is_error: bool) {
        spawn_tool_record(
            self.meter.store.clone(),
            ToolUsageRecord {
                session_id: self.meter.session_id.clone(),
                tool_name: name,
                tool_kind: ToolKind::Server,
                duration_ms: since.map(|s| s.elapsed().as_millis() as u64),
                is_error,
                at_ms: crate::now_ms(),
            },
        );
    }

    fn finish(&mut self) -> Option<JoinHandle<()>> {
        if std::mem::replace(&mut self.done, true) {
            return None;
        }
        // Uses whose result the provider never reports (duration unknown).
        for (name, _) in std::mem::take(&mut self.server_tools).into_values() {
            self.record_server_tool(name, None, false);
        }
        // Without a usage report there is nothing to bill.
        let usage = self.usage.take()?;
        let record = LlmUsageRecord {
            session_id: self.meter.session_id.clone(),
            provider_id: self.meter.provider_id.clone(),
            vendor: self.meter.vendor.clone(),
            model: self.model.clone(),
            purpose: self.purpose,
            tokens: TokenCounts::from_usage(&usage, self.meter.kind),
            ttft_ms: self.ttft_ms,
            duration_ms: Some(self.started.elapsed().as_millis() as u64),
            at_ms: crate::now_ms(),
        };
        let store = self.meter.store.clone();
        let handle = tokio::runtime::Handle::try_current().ok()?;
        Some(handle.spawn(async move {
            if let Err(e) = store.record_llm_call(&record).await {
                tracing::warn!(error = %e, model = %record.model, "usage not recorded");
            }
        }))
    }
}

impl Drop for Call {
    fn drop(&mut self) {
        let _ = self.finish();
    }
}

fn spawn_tool_record(store: Arc<dyn AnalyticsStore>, record: ToolUsageRecord) {
    let Ok(handle) = tokio::runtime::Handle::try_current() else {
        return;
    };
    handle.spawn(async move {
        if let Err(e) = store.record_tool_call(&record).await {
            tracing::warn!(error = %e, tool = %record.tool_name, "tool usage not recorded");
        }
    });
}

pub struct MeteredExecutor {
    inner: Arc<dyn ToolExecutor>,
    store: Arc<dyn AnalyticsStore>,
    session_id: String,
    /// `{server}__` prefixes of the session's MCP servers.
    mcp_prefixes: Vec<String>,
}

impl MeteredExecutor {
    pub fn wrap<'a>(
        inner: Arc<dyn ToolExecutor>,
        store: Arc<dyn AnalyticsStore>,
        session_id: &str,
        mcp_servers: impl IntoIterator<Item = &'a str>,
    ) -> Arc<dyn ToolExecutor> {
        Arc::new(Self {
            inner,
            store,
            session_id: session_id.to_string(),
            mcp_prefixes: mcp_servers.into_iter().map(|n| format!("{n}__")).collect(),
        })
    }
}

#[async_trait]
impl ToolExecutor for MeteredExecutor {
    fn tools(&self) -> Vec<Arc<dyn Tool>> {
        self.inner.tools()
    }

    async fn execute(&self, call: &ToolCall, ctx: ToolContext) -> Result<ToolOutput, ToolError> {
        let started = Instant::now();
        let result = self.inner.execute(call, ctx).await;
        // A call the user interrupted says nothing about the tool.
        if !matches!(result, Err(ToolError::Cancelled)) {
            let mcp = self.mcp_prefixes.iter().any(|p| call.name.starts_with(p));
            let record = ToolUsageRecord {
                session_id: Some(self.session_id.clone()),
                tool_name: call.name.clone(),
                tool_kind: if mcp {
                    ToolKind::Mcp
                } else {
                    ToolKind::Builtin
                },
                duration_ms: Some(started.elapsed().as_millis() as u64),
                is_error: result.as_ref().map_or(true, |o| o.is_error),
                at_ms: crate::now_ms(),
            };
            if let Err(e) = self.store.record_tool_call(&record).await {
                tracing::warn!(error = %e, tool = %record.tool_name, "tool usage not recorded");
            }
        }
        result
    }
}
