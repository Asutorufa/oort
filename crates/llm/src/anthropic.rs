use std::collections::{BTreeMap, VecDeque};

use futures_util::{Stream, StreamExt};
use reqwest::Client as HttpClient;
use reqwest::header::HeaderMap;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::Error;
use crate::client::{ChatStream, LlmClient};
use crate::error::extract_error_message_from_value;
use crate::http::{apply_headers, bytes_stream, ensure_success, insert_header, normalize_base_url};
use crate::sse::parse_sse_stream;
use crate::types::{
    CacheBreakpoint, CacheTtl, ChatRequest, ChatResponse, ContentPart, DocumentSource, ImageSource,
    Message, PromptCacheConfig, Role, StopReason, StreamEvent, ToolCall, ToolChoice,
    ToolDefinition, Usage,
};

pub const DEFAULT_ANTHROPIC_BASE_URL: &str = "https://api.anthropic.com";
pub const DEFAULT_ANTHROPIC_VERSION: &str = "2023-06-01";
const DEFAULT_MAX_TOKENS: u32 = 4096;
const DEFAULT_THINKING_BUDGET_TOKENS: u32 = 1024;
/// Hard upper bound on `cache_control` blocks per Anthropic request.
const ANTHROPIC_MAX_CACHE_BREAKPOINTS: usize = 4;

/// HTTP client for the Anthropic Messages API (`/v1/messages`, `/v1/models`).
#[derive(Debug, Clone)]
pub struct AnthropicClient {
    http: HttpClient,
    base_url: String,
    api_key: String,
    model: String,
    anthropic_version: String,
    extra_headers: HeaderMap,
}

impl AnthropicClient {
    pub fn new(api_key: impl Into<String>, model: impl Into<String>) -> Self {
        Self {
            http: HttpClient::new(),
            base_url: DEFAULT_ANTHROPIC_BASE_URL.to_string(),
            api_key: api_key.into(),
            model: model.into(),
            anthropic_version: DEFAULT_ANTHROPIC_VERSION.to_string(),
            extra_headers: HeaderMap::new(),
        }
    }

    pub fn with_base_url(mut self, base_url: impl Into<String>) -> Self {
        self.base_url = base_url.into();
        self
    }

    pub fn with_version(mut self, version: impl Into<String>) -> Self {
        self.anthropic_version = version.into();
        self
    }

    pub fn with_http_client(mut self, http: HttpClient) -> Self {
        self.http = http;
        self
    }

    pub fn with_header(mut self, name: &str, value: &str) -> Result<Self, Error> {
        insert_header(&mut self.extra_headers, name, value)?;
        Ok(self)
    }

    pub fn model(&self) -> &str {
        &self.model
    }

    pub fn set_model(&mut self, model: impl Into<String>) {
        self.model = model.into();
    }

    fn messages_url(&self) -> String {
        let base = normalize_base_url(&self.base_url, DEFAULT_ANTHROPIC_BASE_URL);
        if base.ends_with("/messages") {
            base.to_string()
        } else if base.ends_with("/v1") {
            format!("{base}/messages")
        } else {
            format!("{base}/v1/messages")
        }
    }

    fn models_url(&self) -> String {
        let base = normalize_base_url(&self.base_url, DEFAULT_ANTHROPIC_BASE_URL);
        if base.ends_with("/models") {
            base.to_string()
        } else if let Some(stripped) = base.strip_suffix("/messages") {
            format!("{stripped}/models")
        } else if base.ends_with("/v1") {
            format!("{base}/models")
        } else {
            format!("{base}/v1/models")
        }
    }

    pub(crate) fn build_request_body(
        &self,
        request: &ChatRequest,
        stream: bool,
    ) -> Result<AnthropicMessagesRequest, Error> {
        let model = request.resolve_model(&self.model)?.to_string();
        let (system_text, _) = request.split_system_messages();
        let cache_control = build_cache_control(request.prompt_cache.as_ref());

        // Tools: preserve exact order; attach cache_control to the last tool when caching is enabled.
        let mut tools = if request.tools.is_empty() {
            None
        } else {
            Some(
                request
                    .tools
                    .iter()
                    .map(encode_anthropic_tool)
                    .collect::<Vec<_>>(),
            )
        };
        // Explicit breakpoints override the default placement (last tool + system + latest tail).
        let explicit = !request.cache_breakpoints.is_empty();
        let static_breakpoint = !explicit
            || request
                .cache_breakpoints
                .contains(&CacheBreakpoint::ToolsAndSystem);

        if static_breakpoint
            && let (Some(cc), Some(tool_list)) = (&cache_control, tools.as_mut())
            && let Some(last_tool) = tool_list.last_mut()
            && let Some(obj) = last_tool.as_object_mut()
        {
            obj.insert("cache_control".to_string(), cc.clone());
        }

        // System: use block array with cache_control when caching is enabled; otherwise plain string.
        let system_has_breakpoint = static_breakpoint && system_text.is_some();
        let system = system_text.map(|text| {
            if let (Some(cc), true) = (&cache_control, static_breakpoint) {
                json!([{
                    "type": "text",
                    "text": text,
                    "cache_control": cc
                }])
            } else {
                Value::String(text)
            }
        });

        // Messages: merge consecutive same-role turns (Anthropic requires alternating user/assistant).
        // `block_pos[i]` records where `request.messages[i]` ends: (merged message index, block index).
        let mut messages: Vec<AnthropicMessage> = Vec::new();
        let mut block_pos: Vec<Option<(usize, usize)>> = vec![None; request.messages.len()];
        for (orig_idx, msg) in request.messages.iter().enumerate() {
            if msg.role == Role::System {
                continue;
            }
            let (role, blocks) = encode_anthropic_message(msg);
            if blocks.is_empty() {
                continue;
            }
            if let Some(last) = messages.last_mut()
                && last.role == role
            {
                last.content.extend(blocks);
            } else {
                messages.push(AnthropicMessage {
                    role,
                    content: blocks,
                });
            }
            let m = messages.len() - 1;
            block_pos[orig_idx] = Some((m, messages[m].content.len() - 1));
        }

        if let Some(cc) = &cache_control {
            let targets: Vec<(usize, usize)> = if explicit {
                // Anthropic allows at most 4 breakpoints; keep the latest message boundaries.
                let used = if static_breakpoint {
                    usize::from(tools.is_some()) + usize::from(system_has_breakpoint)
                } else {
                    0
                };
                let mut idxs: Vec<usize> = request
                    .cache_breakpoints
                    .iter()
                    .filter_map(|bp| match bp {
                        CacheBreakpoint::AfterMessage { index } => Some(*index),
                        CacheBreakpoint::ToolsAndSystem => None,
                    })
                    .collect();
                idxs.sort_unstable();
                idxs.dedup();
                let mut pos: Vec<(usize, usize)> = idxs
                    .into_iter()
                    .filter_map(|i| block_pos.get(i).copied().flatten())
                    .collect();
                pos.dedup();
                let keep = ANTHROPIC_MAX_CACHE_BREAKPOINTS.saturating_sub(used);
                let skip = pos.len().saturating_sub(keep);
                pos.into_iter().skip(skip).collect()
            } else {
                // Rolling conversation prefix breakpoint on the last block of the latest message.
                // Total: at most 3 (last tool + system + latest tail) <= Anthropic's limit of 4.
                messages
                    .last()
                    .map(|m| (messages.len() - 1, m.content.len() - 1))
                    .into_iter()
                    .collect()
            };
            for (m, b) in targets {
                if let Some(obj) = messages[m].content[b].as_object_mut() {
                    obj.insert("cache_control".to_string(), cc.clone());
                }
            }
        }

        let tool_choice = if tools.is_some() {
            request
                .tool_choice
                .as_ref()
                .map(encode_anthropic_tool_choice)
        } else {
            None
        };

        let thinking = request.thinking.as_ref().map(|t| {
            if t.enabled {
                AnthropicThinkingConfig::Enabled {
                    budget_tokens: t
                        .budget_tokens
                        .or(t.effort.map(anthropic_budget))
                        .unwrap_or(DEFAULT_THINKING_BUDGET_TOKENS),
                }
            } else {
                AnthropicThinkingConfig::Disabled
            }
        });
        let thinking_on = matches!(thinking, Some(AnthropicThinkingConfig::Enabled { .. }));

        // `max_tokens` must exceed the thinking budget.
        let max_tokens = match (request.max_tokens, &thinking) {
            (Some(mt), Some(AnthropicThinkingConfig::Enabled { budget_tokens }))
                if mt <= *budget_tokens =>
            {
                budget_tokens.saturating_add(DEFAULT_MAX_TOKENS)
            }
            (Some(mt), _) => mt,
            (None, Some(AnthropicThinkingConfig::Enabled { budget_tokens })) => (*budget_tokens)
                .saturating_add(DEFAULT_MAX_TOKENS)
                .max(16384),
            (None, _) => DEFAULT_MAX_TOKENS,
        };

        let stop_sequences = if request.stop_sequences.is_empty() {
            None
        } else {
            Some(request.stop_sequences.clone())
        };

        Ok(AnthropicMessagesRequest {
            model,
            max_tokens,
            system,
            messages,
            tools,
            tool_choice,
            // Extended thinking rejects a custom temperature / top_k (and top_p below 0.95).
            temperature: request.temperature.filter(|_| !thinking_on),
            top_p: request.top_p.filter(|p| !thinking_on || *p >= 0.95),
            top_k: request.top_k.filter(|_| !thinking_on),
            stop_sequences,
            thinking,
            stream: Some(stream),
        })
    }
}

/// Thinking budget for an effort level.
fn anthropic_budget(effort: crate::ReasoningEffort) -> u32 {
    match effort {
        crate::ReasoningEffort::Low => 4_096,
        crate::ReasoningEffort::Medium => 10_240,
        crate::ReasoningEffort::High => 24_576,
    }
}

impl LlmClient for AnthropicClient {
    async fn chat(&self, request: ChatRequest) -> Result<ChatResponse, Error> {
        let body = self.build_request_body(&request, false)?;

        let mut req_builder = self
            .http
            .post(self.messages_url())
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", &self.anthropic_version)
            .header("Content-Type", "application/json")
            .json(&body);
        req_builder = apply_headers(req_builder, &self.extra_headers);

        let resp = ensure_success(req_builder.send().await?).await?;
        let api_resp: AnthropicMessagesResponse = resp.json().await?;
        Ok(parse_anthropic_response(api_resp))
    }

    async fn chat_stream(&self, request: ChatRequest) -> Result<ChatStream, Error> {
        let body = self.build_request_body(&request, true)?;

        let mut req_builder = self
            .http
            .post(self.messages_url())
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", &self.anthropic_version)
            .header("Content-Type", "application/json")
            .header("Accept", "text/event-stream")
            .json(&body);
        req_builder = apply_headers(req_builder, &self.extra_headers);

        let resp = ensure_success(req_builder.send().await?).await?;
        let sse = parse_sse_stream(bytes_stream(resp));
        Ok(Box::pin(anthropic_sse_to_events(sse)))
    }

    async fn list_models(&self) -> Result<Vec<String>, Error> {
        let models_url = self.models_url();
        let mut models = Vec::new();
        let mut after_id: Option<String> = None;

        loop {
            let mut request = self
                .http
                .get(&models_url)
                .query(&[("limit", "1000")])
                .header("x-api-key", &self.api_key)
                .header("anthropic-version", &self.anthropic_version);
            if let Some(cursor) = &after_id {
                request = request.query(&[("after_id", cursor.as_str())]);
            }
            request = apply_headers(request, &self.extra_headers);

            let response = ensure_success(request.send().await?).await?;
            let page: AnthropicModelsResponse = response.json().await?;

            let next_after_id = page
                .last_id
                .clone()
                .or_else(|| page.data.last().map(|m| m.id.trim().to_string()));

            models.extend(
                page.data
                    .into_iter()
                    .map(|m| m.id.trim().to_string())
                    .filter(|id| !id.is_empty()),
            );

            if !page.has_more {
                break;
            }
            let Some(next) = next_after_id.filter(|id| !id.is_empty()) else {
                break;
            };
            if after_id.as_deref() == Some(next.as_str()) {
                break;
            }
            after_id = Some(next);
        }

        Ok(models)
    }
}

// --- Wire types ---

#[derive(Debug, Serialize)]
pub(crate) struct AnthropicMessagesRequest {
    model: String,
    max_tokens: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    system: Option<Value>,
    messages: Vec<AnthropicMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<Vec<Value>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_choice: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    top_p: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    top_k: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    stop_sequences: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    thinking: Option<AnthropicThinkingConfig>,
    #[serde(skip_serializing_if = "Option::is_none")]
    stream: Option<bool>,
}

#[derive(Debug, Serialize)]
struct AnthropicMessage {
    role: &'static str,
    content: Vec<Value>,
}

#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "lowercase")]
enum AnthropicThinkingConfig {
    Enabled { budget_tokens: u32 },
    Disabled,
}

#[derive(Debug, Deserialize)]
struct AnthropicMessagesResponse {
    id: Option<String>,
    model: Option<String>,
    #[serde(default)]
    content: Vec<Value>,
    stop_reason: Option<String>,
    usage: Option<AnthropicUsage>,
}

#[derive(Debug, Deserialize, Default)]
struct AnthropicUsage {
    #[serde(default)]
    input_tokens: u64,
    #[serde(default)]
    output_tokens: u64,
    cache_read_input_tokens: Option<u64>,
    cache_creation_input_tokens: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct AnthropicModelsResponse {
    #[serde(default)]
    data: Vec<AnthropicModel>,
    #[serde(default)]
    has_more: bool,
    last_id: Option<String>,
}

#[derive(Debug, Deserialize)]
struct AnthropicModel {
    id: String,
}

// --- Encoding & Decoding helpers ---

fn build_cache_control(cache: Option<&PromptCacheConfig>) -> Option<Value> {
    let cache = cache.filter(|c| c.enabled)?;
    match cache.ttl {
        Some(CacheTtl::OneHour) => Some(json!({ "type": "ephemeral", "ttl": "1h" })),
        _ => Some(json!({ "type": "ephemeral" })),
    }
}

fn encode_anthropic_tool(tool: &ToolDefinition) -> Value {
    json!({
        "name": tool.name,
        "description": tool.description,
        "input_schema": tool.parameters,
    })
}

fn encode_anthropic_tool_choice(choice: &ToolChoice) -> Value {
    match choice {
        ToolChoice::Auto => json!({ "type": "auto" }),
        ToolChoice::None => json!({ "type": "none" }),
        ToolChoice::Required => json!({ "type": "any" }),
        ToolChoice::Tool { name } => json!({ "type": "tool", "name": name }),
    }
}

fn encode_anthropic_message(msg: &Message) -> (&'static str, Vec<Value>) {
    let role = match msg.role {
        Role::Assistant => "assistant",
        Role::User | Role::Tool | Role::System => "user",
    };

    let mut blocks = Vec::new();
    for part in &msg.content {
        match part {
            ContentPart::Text { text } => {
                blocks.push(json!({ "type": "text", "text": text }));
            }
            ContentPart::Image { source } => match source {
                ImageSource::Base64 { media_type, data } => {
                    blocks.push(json!({
                        "type": "image",
                        "source": {
                            "type": "base64",
                            "media_type": media_type,
                            "data": data,
                        }
                    }));
                }
                ImageSource::Url { url } => {
                    blocks.push(json!({
                        "type": "image",
                        "source": {
                            "type": "url",
                            "url": url,
                        }
                    }));
                }
            },
            ContentPart::Document { source, filename } => {
                let source_val = match source {
                    DocumentSource::Base64 { media_type, data } => json!({
                        "type": "base64",
                        "media_type": media_type,
                        "data": data,
                    }),
                    DocumentSource::Url { url, .. } => json!({
                        "type": "url",
                        "url": url,
                    }),
                    DocumentSource::FileId { file_id, .. } => json!({
                        "type": "file",
                        "file_id": file_id,
                    }),
                };
                let mut obj = serde_json::Map::new();
                obj.insert("type".to_string(), Value::String("document".to_string()));
                obj.insert("source".to_string(), source_val);
                if let Some(title) = filename {
                    obj.insert("title".to_string(), Value::String(title.clone()));
                }
                blocks.push(Value::Object(obj));
            }
            ContentPart::Thinking {
                thinking,
                signature,
            } => {
                let mut obj = serde_json::Map::new();
                obj.insert("type".to_string(), Value::String("thinking".to_string()));
                obj.insert("thinking".to_string(), Value::String(thinking.clone()));
                if let Some(sig) = signature {
                    obj.insert("signature".to_string(), Value::String(sig.clone()));
                }
                blocks.push(Value::Object(obj));
            }
            ContentPart::ToolCall(call) => {
                blocks.push(json!({
                    "type": "tool_use",
                    "id": call.id,
                    "name": call.name,
                    "input": call.arguments,
                }));
            }
            ContentPart::ToolResult(result) => {
                let mut obj = serde_json::Map::new();
                obj.insert("type".to_string(), Value::String("tool_result".to_string()));
                obj.insert(
                    "tool_use_id".to_string(),
                    Value::String(result.tool_call_id.clone()),
                );
                obj.insert("content".to_string(), Value::String(result.content.clone()));
                if result.is_error {
                    obj.insert("is_error".to_string(), Value::Bool(true));
                }
                blocks.push(Value::Object(obj));
            }
        }
    }

    (role, blocks)
}

fn map_anthropic_stop_reason(reason: &str) -> StopReason {
    match reason {
        "end_turn" => StopReason::EndTurn,
        "max_tokens" => StopReason::MaxTokens,
        "tool_use" => StopReason::ToolUse,
        "stop_sequence" => StopReason::StopSequence,
        "refusal" => StopReason::ContentFilter,
        other => StopReason::Other(other.to_string()),
    }
}

fn map_anthropic_usage(u: &AnthropicUsage) -> Usage {
    Usage {
        input_tokens: u.input_tokens,
        output_tokens: u.output_tokens,
        total_tokens: Some(u.input_tokens.saturating_add(u.output_tokens)),
        reasoning_tokens: None,
        cache_read_tokens: u.cache_read_input_tokens,
        cache_creation_tokens: u.cache_creation_input_tokens,
    }
}

fn parse_anthropic_response(resp: AnthropicMessagesResponse) -> ChatResponse {
    let mut content = String::new();
    let mut thinking: Option<String> = None;
    let mut thinking_signature: Option<String> = None;
    let mut tool_calls = Vec::new();

    for block in resp.content {
        match block.get("type").and_then(Value::as_str) {
            Some("text") => {
                if let Some(text) = block.get("text").and_then(Value::as_str) {
                    content.push_str(text);
                }
            }
            Some("thinking") => {
                if let Some(t) = block.get("thinking").and_then(Value::as_str)
                    && !t.is_empty()
                {
                    thinking.get_or_insert_with(String::new).push_str(t);
                }
                if let Some(sig) = block.get("signature").and_then(Value::as_str)
                    && !sig.is_empty()
                {
                    thinking_signature
                        .get_or_insert_with(String::new)
                        .push_str(sig);
                }
            }
            Some("tool_use") => {
                let id = block
                    .get("id")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                let name = block
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                let input = block.get("input").cloned().unwrap_or_else(|| json!({}));
                tool_calls.push(ToolCall {
                    id,
                    name,
                    arguments: input,
                    signature: None,
                });
            }
            _ => {}
        }
    }

    ChatResponse {
        id: resp.id,
        model: resp.model,
        content,
        thinking,
        thinking_signature,
        tool_calls,
        stop_reason: resp.stop_reason.as_deref().map(map_anthropic_stop_reason),
        usage: resp.usage.as_ref().map(map_anthropic_usage),
    }
}

struct AnthropicStreamState<S> {
    sse: S,
    queue: VecDeque<Result<StreamEvent, Error>>,
    /// Maps Anthropic content block index -> tool call ordinal index.
    tool_block_indices: BTreeMap<usize, usize>,
    next_tool_index: usize,
    usage: AnthropicUsage,
    has_usage: bool,
    stop_reason: Option<StopReason>,
    done: bool,
}

fn anthropic_sse_to_events<S>(
    sse: S,
) -> impl Stream<Item = Result<StreamEvent, Error>> + Send + 'static
where
    S: Stream<Item = Result<crate::sse::SseEvent, Error>> + Send + Unpin + 'static,
{
    let state = AnthropicStreamState {
        sse,
        queue: VecDeque::new(),
        tool_block_indices: BTreeMap::new(),
        next_tool_index: 0,
        usage: AnthropicUsage::default(),
        has_usage: false,
        stop_reason: None,
        done: false,
    };

    futures_util::stream::unfold(state, |mut st| async move {
        loop {
            if let Some(item) = st.queue.pop_front() {
                return Some((item, st));
            }
            if st.done {
                return None;
            }

            match st.sse.next().await {
                Some(Ok(event)) => {
                    let data = event.data.trim();
                    if data.is_empty() {
                        continue;
                    }
                    if data == "[DONE]" {
                        finish_anthropic_stream(&mut st);
                        continue;
                    }

                    let value: Value = match serde_json::from_str(data) {
                        Ok(v) => v,
                        Err(e) => {
                            st.done = true;
                            return Some((Err(Error::Serialization(e)), st));
                        }
                    };

                    let event_type = value
                        .get("type")
                        .and_then(Value::as_str)
                        .or(event.event.as_deref());

                    match event_type {
                        Some("message_start") => {
                            let msg = value.get("message");
                            let id = msg
                                .and_then(|m| m.get("id"))
                                .and_then(Value::as_str)
                                .map(ToString::to_string);
                            let model = msg
                                .and_then(|m| m.get("model"))
                                .and_then(Value::as_str)
                                .map(ToString::to_string);
                            if let Some(usage_val) = msg.and_then(|m| m.get("usage"))
                                && let Ok(u) =
                                    serde_json::from_value::<AnthropicUsage>(usage_val.clone())
                            {
                                st.usage = u;
                                st.has_usage = true;
                            }
                            st.queue
                                .push_back(Ok(StreamEvent::MessageStart { id, model }));
                        }
                        Some("content_block_start") => {
                            let block_idx =
                                value.get("index").and_then(Value::as_u64).unwrap_or(0) as usize;
                            if let Some(block) = value.get("content_block") {
                                match block.get("type").and_then(Value::as_str) {
                                    Some("tool_use") => {
                                        let tool_idx = st.next_tool_index;
                                        st.next_tool_index += 1;
                                        st.tool_block_indices.insert(block_idx, tool_idx);
                                        let id = block
                                            .get("id")
                                            .and_then(Value::as_str)
                                            .unwrap_or_default()
                                            .to_string();
                                        let name = block
                                            .get("name")
                                            .and_then(Value::as_str)
                                            .unwrap_or_default()
                                            .to_string();
                                        st.queue.push_back(Ok(StreamEvent::ToolCallStart {
                                            index: tool_idx,
                                            id,
                                            name,
                                        }));
                                    }
                                    Some("text") => {
                                        if let Some(t) = block.get("text").and_then(Value::as_str)
                                            && !t.is_empty()
                                        {
                                            st.queue.push_back(Ok(StreamEvent::TextDelta(
                                                t.to_string(),
                                            )));
                                        }
                                    }
                                    Some("thinking") => {
                                        if let Some(t) =
                                            block.get("thinking").and_then(Value::as_str)
                                            && !t.is_empty()
                                        {
                                            st.queue.push_back(Ok(StreamEvent::ThinkingDelta(
                                                t.to_string(),
                                            )));
                                        }
                                    }
                                    _ => {}
                                }
                            }
                        }
                        Some("content_block_delta") => {
                            let block_idx =
                                value.get("index").and_then(Value::as_u64).unwrap_or(0) as usize;
                            if let Some(delta) = value.get("delta") {
                                match delta.get("type").and_then(Value::as_str) {
                                    Some("text_delta") => {
                                        if let Some(t) = delta.get("text").and_then(Value::as_str)
                                            && !t.is_empty()
                                        {
                                            st.queue.push_back(Ok(StreamEvent::TextDelta(
                                                t.to_string(),
                                            )));
                                        }
                                    }
                                    Some("thinking_delta") => {
                                        if let Some(t) =
                                            delta.get("thinking").and_then(Value::as_str)
                                            && !t.is_empty()
                                        {
                                            st.queue.push_back(Ok(StreamEvent::ThinkingDelta(
                                                t.to_string(),
                                            )));
                                        }
                                    }
                                    Some("signature_delta") => {
                                        if let Some(sig) =
                                            delta.get("signature").and_then(Value::as_str)
                                            && !sig.is_empty()
                                        {
                                            st.queue.push_back(Ok(
                                                StreamEvent::ThinkingSignatureDelta(
                                                    sig.to_string(),
                                                ),
                                            ));
                                        }
                                    }
                                    Some("input_json_delta") => {
                                        if let Some(partial) =
                                            delta.get("partial_json").and_then(Value::as_str)
                                            && !partial.is_empty()
                                            && let Some(&tool_idx) =
                                                st.tool_block_indices.get(&block_idx)
                                        {
                                            st.queue.push_back(Ok(StreamEvent::ToolCallDelta {
                                                index: tool_idx,
                                                arguments_delta: partial.to_string(),
                                            }));
                                        }
                                    }
                                    _ => {}
                                }
                            }
                        }
                        Some("content_block_stop") => {
                            let block_idx =
                                value.get("index").and_then(Value::as_u64).unwrap_or(0) as usize;
                            if let Some(tool_idx) = st.tool_block_indices.remove(&block_idx) {
                                st.queue
                                    .push_back(Ok(StreamEvent::ToolCallEnd { index: tool_idx }));
                            }
                        }
                        Some("message_delta") => {
                            if let Some(reason) =
                                value.pointer("/delta/stop_reason").and_then(Value::as_str)
                            {
                                st.stop_reason = Some(map_anthropic_stop_reason(reason));
                            }
                            if let Some(usage_val) = value.get("usage") {
                                if let Some(out) =
                                    usage_val.get("output_tokens").and_then(Value::as_u64)
                                {
                                    st.usage.output_tokens = out;
                                    st.has_usage = true;
                                }
                                if let Some(inp) =
                                    usage_val.get("input_tokens").and_then(Value::as_u64)
                                {
                                    st.usage.input_tokens = inp;
                                    st.has_usage = true;
                                }
                                if let Some(cr) = usage_val
                                    .get("cache_read_input_tokens")
                                    .and_then(Value::as_u64)
                                {
                                    st.usage.cache_read_tokens_set(cr);
                                    st.has_usage = true;
                                }
                                if let Some(cc) = usage_val
                                    .get("cache_creation_input_tokens")
                                    .and_then(Value::as_u64)
                                {
                                    st.usage.cache_creation_tokens_set(cc);
                                    st.has_usage = true;
                                }
                            }
                        }
                        Some("message_stop") => {
                            finish_anthropic_stream(&mut st);
                        }
                        Some("error") => {
                            let msg = extract_error_message_from_value(&value)
                                .unwrap_or_else(|| "Anthropic stream error".to_string());
                            st.done = true;
                            return Some((Err(Error::api(msg)), st));
                        }
                        _ => {}
                    }
                }
                Some(Err(err)) => {
                    st.done = true;
                    return Some((Err(err), st));
                }
                None => {
                    finish_anthropic_stream(&mut st);
                }
            }
        }
    })
}

impl AnthropicUsage {
    fn cache_read_tokens_set(&mut self, value: u64) {
        self.cache_read_input_tokens = Some(value);
    }

    fn cache_creation_tokens_set(&mut self, value: u64) {
        self.cache_creation_input_tokens = Some(value);
    }
}

fn finish_anthropic_stream<S>(st: &mut AnthropicStreamState<S>) {
    st.done = true;
    let remaining = std::mem::take(&mut st.tool_block_indices);
    for (_, tool_idx) in remaining {
        st.queue
            .push_back(Ok(StreamEvent::ToolCallEnd { index: tool_idx }));
    }
    if st.has_usage {
        st.queue
            .push_back(Ok(StreamEvent::Usage(map_anthropic_usage(&st.usage))));
    }
    st.queue.push_back(Ok(StreamEvent::Done {
        stop_reason: st.stop_reason.take(),
    }));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::collect_stream;
    use mockito::Server;

    #[tokio::test]
    async fn test_anthropic_chat_thinking_and_tool_use() {
        let mut server = Server::new_async().await;
        let mock = server
            .mock("POST", "/v1/messages")
            .match_header("x-api-key", "sk-ant")
            .match_header("anthropic-version", "2023-06-01")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                r#"{
                    "id": "msg_1",
                    "type": "message",
                    "role": "assistant",
                    "model": "claude-sonnet-4-20250514",
                    "content": [
                        {"type": "thinking", "thinking": "Let me check", "signature": "sig_xyz"},
                        {"type": "text", "text": "Calling weather tool."},
                        {"type": "tool_use", "id": "toolu_1", "name": "weather", "input": {"city": "Paris"}}
                    ],
                    "stop_reason": "tool_use",
                    "usage": {
                        "input_tokens": 100,
                        "output_tokens": 50,
                        "cache_read_input_tokens": 80,
                        "cache_creation_input_tokens": 20
                    }
                }"#,
            )
            .create_async()
            .await;

        let client =
            AnthropicClient::new("sk-ant", "claude-sonnet-4-20250514").with_base_url(server.url());
        let req = ChatRequest::new(vec![Message::user("Weather in Paris?")])
            .with_system("Be concise")
            .with_thinking(crate::ThinkingConfig::with_budget(2048))
            .with_prompt_cache(PromptCacheConfig::enabled());

        let resp = client.chat(req).await.unwrap();
        mock.assert_async().await;

        assert_eq!(resp.id.as_deref(), Some("msg_1"));
        assert_eq!(resp.thinking.as_deref(), Some("Let me check"));
        assert_eq!(resp.thinking_signature.as_deref(), Some("sig_xyz"));
        assert_eq!(resp.content, "Calling weather tool.");
        assert_eq!(resp.tool_calls.len(), 1);
        assert_eq!(resp.tool_calls[0].id, "toolu_1");
        assert_eq!(resp.tool_calls[0].arguments, json!({"city": "Paris"}));
        assert_eq!(resp.stop_reason, Some(StopReason::ToolUse));

        let usage = resp.usage.unwrap();
        assert_eq!(usage.input_tokens, 100);
        assert_eq!(usage.output_tokens, 50);
        assert_eq!(usage.cache_read_tokens, Some(80));
        assert_eq!(usage.cache_creation_tokens, Some(20));
    }

    #[tokio::test]
    async fn test_anthropic_stream_thinking_signature_text_and_tool_use() {
        let mut server = Server::new_async().await;
        let body = concat!(
            "event: message_start\n",
            "data: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_s1\",\"model\":\"claude-sonnet-4-20250514\",\"usage\":{\"input_tokens\":120,\"output_tokens\":0,\"cache_read_input_tokens\":100}}}\n\n",
            "event: content_block_start\n",
            "data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"thinking\",\"thinking\":\"\"}}\n\n",
            "event: content_block_delta\n",
            "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"thinking_delta\",\"thinking\":\"Plan\"}}\n\n",
            "event: content_block_delta\n",
            "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"signature_delta\",\"signature\":\"sig_part1\"}}\n\n",
            "event: content_block_stop\n",
            "data: {\"type\":\"content_block_stop\",\"index\":0}\n\n",
            "event: content_block_start\n",
            "data: {\"type\":\"content_block_start\",\"index\":1,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\n",
            "event: content_block_delta\n",
            "data: {\"type\":\"content_block_delta\",\"index\":1,\"delta\":{\"type\":\"text_delta\",\"text\":\"Running...\"}}\n\n",
            "event: content_block_stop\n",
            "data: {\"type\":\"content_block_stop\",\"index\":1}\n\n",
            "event: content_block_start\n",
            "data: {\"type\":\"content_block_start\",\"index\":2,\"content_block\":{\"type\":\"tool_use\",\"id\":\"toolu_9\",\"name\":\"run_cmd\",\"input\":{}}}\n\n",
            "event: content_block_delta\n",
            "data: {\"type\":\"content_block_delta\",\"index\":2,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"{\\\"cmd\\\":\"}}\n\n",
            "event: content_block_delta\n",
            "data: {\"type\":\"content_block_delta\",\"index\":2,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"\\\"ls\\\"}\"}}\n\n",
            "event: content_block_stop\n",
            "data: {\"type\":\"content_block_stop\",\"index\":2}\n\n",
            "event: message_delta\n",
            "data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"tool_use\"},\"usage\":{\"output_tokens\":45}}\n\n",
            "event: message_stop\n",
            "data: {\"type\":\"message_stop\"}\n\n"
        );

        let mock = server
            .mock("POST", "/v1/messages")
            .match_header("accept", "text/event-stream")
            .with_status(200)
            .with_header("content-type", "text/event-stream")
            .with_body(body)
            .create_async()
            .await;

        let client =
            AnthropicClient::new("sk-ant", "claude-sonnet-4-20250514").with_base_url(server.url());
        let stream = client
            .chat_stream(ChatRequest::new(vec![Message::user("list files")]))
            .await
            .unwrap();
        let resp = collect_stream(stream).await.unwrap();
        mock.assert_async().await;

        assert_eq!(resp.id.as_deref(), Some("msg_s1"));
        assert_eq!(resp.thinking.as_deref(), Some("Plan"));
        assert_eq!(resp.thinking_signature.as_deref(), Some("sig_part1"));
        assert_eq!(resp.content, "Running...");
        assert_eq!(resp.tool_calls.len(), 1);
        assert_eq!(resp.tool_calls[0].id, "toolu_9");
        assert_eq!(resp.tool_calls[0].name, "run_cmd");
        assert_eq!(resp.tool_calls[0].arguments, json!({"cmd": "ls"}));
        assert_eq!(resp.stop_reason, Some(StopReason::ToolUse));
        let usage = resp.usage.unwrap();
        assert_eq!(usage.input_tokens, 120);
        assert_eq!(usage.output_tokens, 45);
        assert_eq!(usage.cache_read_tokens, Some(100));
    }

    #[tokio::test]
    async fn test_anthropic_list_models_pagination() {
        let mut server = Server::new_async().await;
        let page1 = server
            .mock("GET", "/v1/models?limit=1000")
            .match_header("x-api-key", "sk-ant")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"data":[{"id":"claude-1"}],"has_more":true,"last_id":"claude-1"}"#)
            .create_async()
            .await;
        let page2 = server
            .mock("GET", "/v1/models?limit=1000&after_id=claude-1")
            .match_header("x-api-key", "sk-ant")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"data":[{"id":"claude-2"}],"has_more":false,"last_id":"claude-2"}"#)
            .create_async()
            .await;

        let client = AnthropicClient::new("sk-ant", "claude-1").with_base_url(server.url());
        let models = client.list_models().await.unwrap();
        page1.assert_async().await;
        page2.assert_async().await;
        assert_eq!(models, vec!["claude-1", "claude-2"]);
    }

    #[tokio::test]
    async fn test_anthropic_stream_error_event() {
        let mut server = Server::new_async().await;
        let body = "event: error\ndata: {\"type\":\"error\",\"error\":{\"type\":\"overloaded_error\",\"message\":\"Overloaded\"}}\n\n";
        let mock = server
            .mock("POST", "/v1/messages")
            .with_status(200)
            .with_header("content-type", "text/event-stream")
            .with_body(body)
            .create_async()
            .await;

        let client =
            AnthropicClient::new("sk-ant", "claude-sonnet-4-20250514").with_base_url(server.url());
        let stream = client
            .chat_stream(ChatRequest::new(vec![Message::user("hi")]))
            .await
            .unwrap();
        let err = collect_stream(stream).await.unwrap_err();
        mock.assert_async().await;
        assert!(err.to_string().contains("overloaded_error: Overloaded"));
    }
}
