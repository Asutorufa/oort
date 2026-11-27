use std::collections::{BTreeSet, VecDeque};

use futures_util::{Stream, StreamExt};
use reqwest::Client as HttpClient;
use reqwest::header::HeaderMap;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::client::{ChatStream, LlmClient};
use crate::error::extract_error_message_from_value;
use crate::http::{apply_headers, bytes_stream, ensure_success, insert_header, normalize_base_url};
use crate::sse::parse_sse_stream;
use crate::types::{
    CacheTtl, ChatRequest, ChatResponse, ContentPart, ImageSource, Message, ReasoningEffort, Role,
    StopReason, StreamEvent, ToolCall, ToolChoice, ToolDefinition, Usage, parse_tool_arguments,
};
use crate::Error;

pub const DEFAULT_OPENAI_BASE_URL: &str = "https://api.openai.com/v1";

/// HTTP client for the OpenAI Chat Completions API (and OpenAI-compatible endpoints).
#[derive(Debug, Clone)]
pub struct OpenAIClient {
    http: HttpClient,
    base_url: String,
    api_key: String,
    model: String,
    extra_headers: HeaderMap,
    include_stream_usage: bool,
}

impl OpenAIClient {
    /// Creates a new OpenAI Chat Completions client pointing at `https://api.openai.com/v1`.
    pub fn new(api_key: impl Into<String>, model: impl Into<String>) -> Self {
        Self {
            http: HttpClient::new(),
            base_url: DEFAULT_OPENAI_BASE_URL.to_string(),
            api_key: api_key.into(),
            model: model.into(),
            extra_headers: HeaderMap::new(),
            include_stream_usage: true,
        }
    }

    pub fn with_base_url(mut self, base_url: impl Into<String>) -> Self {
        self.base_url = base_url.into();
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

    /// Controls whether `stream_options: { "include_usage": true }` is sent on streaming requests.
    /// Defaults to `true`. Disable for OpenAI-compatible servers that reject `stream_options`.
    pub fn with_stream_usage(mut self, include: bool) -> Self {
        self.include_stream_usage = include;
        self
    }

    pub fn model(&self) -> &str {
        &self.model
    }

    pub fn set_model(&mut self, model: impl Into<String>) {
        self.model = model.into();
    }

    pub fn base_url(&self) -> &str {
        normalize_base_url(&self.base_url, DEFAULT_OPENAI_BASE_URL)
    }

    fn chat_completions_url(&self) -> String {
        let base = self.base_url();
        if base.ends_with("/chat/completions") {
            base.to_string()
        } else {
            format!("{base}/chat/completions")
        }
    }

    pub(crate) fn build_request_body(
        &self,
        request: &ChatRequest,
        stream: bool,
    ) -> Result<OpenAIChatRequest, Error> {
        let model = request.resolve_model(&self.model)?.to_string();
        let (system, non_system) = request.split_system_messages();

        let mut messages = Vec::new();
        if let Some(sys) = system {
            messages.push(OpenAIMessage {
                role: "system".to_string(),
                content: Some(Value::String(sys)),
                tool_calls: None,
                tool_call_id: None,
                reasoning_content: None,
            });
        }

        for msg in non_system {
            encode_openai_message(msg, &mut messages);
        }

        let tools = if request.tools.is_empty() {
            None
        } else {
            Some(request.tools.iter().map(encode_openai_tool).collect())
        };

        let tool_choice = if tools.is_some() {
            request.tool_choice.as_ref().map(encode_openai_tool_choice)
        } else {
            None
        };

        let reasoning_effort = request
            .thinking
            .as_ref()
            .filter(|t| t.enabled)
            .and_then(|t| t.effort)
            .map(reasoning_effort_str);

        let stop = if request.stop_sequences.is_empty() {
            None
        } else {
            Some(request.stop_sequences.clone())
        };

        let stream_options = if stream && self.include_stream_usage {
            Some(OpenAIStreamOptions {
                include_usage: true,
            })
        } else {
            None
        };

        let (prompt_cache_key, prompt_cache_retention) =
            extract_openai_cache_fields(request.prompt_cache.as_ref());

        Ok(OpenAIChatRequest {
            model,
            messages,
            tools,
            tool_choice,
            max_tokens: request.max_tokens,
            temperature: request.temperature,
            top_p: request.top_p,
            stop,
            reasoning_effort,
            prompt_cache_key,
            prompt_cache_retention,
            stream: Some(stream),
            stream_options,
        })
    }
}

impl LlmClient for OpenAIClient {
    async fn chat(&self, request: ChatRequest) -> Result<ChatResponse, Error> {
        let body = self.build_request_body(&request, false)?;

        let mut req_builder = self.http.post(self.chat_completions_url()).json(&body);
        if !self.api_key.trim().is_empty() {
            req_builder = req_builder.bearer_auth(&self.api_key);
        }
        req_builder = apply_headers(req_builder, &self.extra_headers);

        let resp = ensure_success(req_builder.send().await?).await?;
        let api_resp: OpenAIChatResponse = resp.json().await?;
        parse_chat_response(api_resp)
    }

    async fn chat_stream(&self, request: ChatRequest) -> Result<ChatStream, Error> {
        let body = self.build_request_body(&request, true)?;

        let mut req_builder = self
            .http
            .post(self.chat_completions_url())
            .header("Accept", "text/event-stream")
            .json(&body);
        if !self.api_key.trim().is_empty() {
            req_builder = req_builder.bearer_auth(&self.api_key);
        }
        req_builder = apply_headers(req_builder, &self.extra_headers);

        let resp = ensure_success(req_builder.send().await?).await?;
        let sse = parse_sse_stream(bytes_stream(resp));
        Ok(Box::pin(openai_sse_to_events(sse)))
    }

    async fn list_models(&self) -> Result<Vec<String>, Error> {
        fetch_openai_models(&self.http, &self.base_url, &self.api_key, &self.extra_headers).await
    }
}

pub(crate) fn openai_models_url(base_url: &str) -> String {
    let base = normalize_base_url(base_url, DEFAULT_OPENAI_BASE_URL);
    let base = base
        .strip_suffix("/chat/completions")
        .or_else(|| base.strip_suffix("/responses"))
        .or_else(|| base.strip_suffix("/models"))
        .unwrap_or(base);
    format!("{base}/models")
}

pub(crate) async fn fetch_openai_models(
    http: &HttpClient,
    base_url: &str,
    api_key: &str,
    extra_headers: &HeaderMap,
) -> Result<Vec<String>, Error> {
    let mut request = http.get(openai_models_url(base_url));
    if !api_key.trim().is_empty() {
        request = request.bearer_auth(api_key);
    }
    request = apply_headers(request, extra_headers);

    let response = ensure_success(request.send().await?).await?;
    let parsed: OpenAIModelsResponse = response.json().await?;
    Ok(parsed
        .data
        .into_iter()
        .map(|m| m.id.trim().to_string())
        .filter(|id| !id.is_empty())
        .collect())
}

// --- Wire types ---

#[derive(Debug, Serialize)]
pub(crate) struct OpenAIChatRequest {
    model: String,
    messages: Vec<OpenAIMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<Vec<Value>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_choice: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    top_p: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    stop: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    reasoning_effort: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    prompt_cache_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    prompt_cache_retention: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    stream: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    stream_options: Option<OpenAIStreamOptions>,
}

#[derive(Debug, Serialize)]
struct OpenAIStreamOptions {
    include_usage: bool,
}

#[derive(Debug, Serialize)]
struct OpenAIMessage {
    role: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    content: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_calls: Option<Vec<OpenAIToolCallWire>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_call_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    reasoning_content: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct OpenAIToolCallWire {
    id: String,
    r#type: String,
    function: OpenAIFunctionCallWire,
}

#[derive(Debug, Serialize, Deserialize)]
struct OpenAIFunctionCallWire {
    name: String,
    arguments: String,
}

#[derive(Debug, Deserialize)]
struct OpenAIChatResponse {
    id: Option<String>,
    model: Option<String>,
    #[serde(default)]
    choices: Vec<OpenAIChoice>,
    usage: Option<OpenAIUsage>,
}

#[derive(Debug, Deserialize)]
struct OpenAIChoice {
    message: OpenAIResponseMessage,
    finish_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
struct OpenAIResponseMessage {
    content: Option<String>,
    reasoning_content: Option<String>,
    reasoning: Option<String>,
    #[serde(default)]
    tool_calls: Vec<OpenAIToolCallWire>,
}

#[derive(Debug, Deserialize)]
struct OpenAIUsage {
    #[serde(default)]
    prompt_tokens: u64,
    #[serde(default)]
    completion_tokens: u64,
    total_tokens: Option<u64>,
    prompt_tokens_details: Option<OpenAIPromptTokensDetails>,
    completion_tokens_details: Option<OpenAICompletionTokensDetails>,
}

#[derive(Debug, Deserialize)]
struct OpenAIPromptTokensDetails {
    cached_tokens: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct OpenAICompletionTokensDetails {
    reasoning_tokens: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct OpenAIStreamChunk {
    id: Option<String>,
    model: Option<String>,
    #[serde(default)]
    choices: Vec<OpenAIStreamChoice>,
    usage: Option<OpenAIUsage>,
}

#[derive(Debug, Deserialize)]
struct OpenAIStreamChoice {
    delta: OpenAIStreamDelta,
    finish_reason: Option<String>,
}

#[derive(Debug, Deserialize, Default)]
struct OpenAIStreamDelta {
    content: Option<String>,
    reasoning_content: Option<String>,
    reasoning: Option<String>,
    thought: Option<String>,
    #[serde(default)]
    tool_calls: Vec<OpenAIStreamToolCallDelta>,
}

#[derive(Debug, Deserialize)]
struct OpenAIStreamToolCallDelta {
    #[serde(default)]
    index: usize,
    id: Option<String>,
    function: Option<OpenAIStreamFunctionDelta>,
}

#[derive(Debug, Deserialize)]
struct OpenAIStreamFunctionDelta {
    name: Option<String>,
    arguments: Option<String>,
}

#[derive(Debug, Deserialize)]
struct OpenAIModelsResponse {
    #[serde(default)]
    data: Vec<OpenAIModel>,
}

#[derive(Debug, Deserialize)]
struct OpenAIModel {
    id: String,
}

// --- Encoding & Decoding helpers ---

pub(crate) fn reasoning_effort_str(effort: ReasoningEffort) -> &'static str {
    match effort {
        ReasoningEffort::Low => "low",
        ReasoningEffort::Medium => "medium",
        ReasoningEffort::High => "high",
    }
}

pub(crate) fn extract_openai_cache_fields(
    cache: Option<&crate::types::PromptCacheConfig>,
) -> (Option<String>, Option<&'static str>) {
    let Some(cache) = cache.filter(|c| c.enabled) else {
        return (None, None);
    };
    let key = cache
        .key
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToString::to_string);
    let retention = match cache.ttl {
        Some(CacheTtl::TwentyFourHours) => Some("24h"),
        _ => None,
    };
    (key, retention)
}

fn encode_openai_tool(tool: &ToolDefinition) -> Value {
    json!({
        "type": "function",
        "function": {
            "name": tool.name,
            "description": tool.description,
            "parameters": tool.parameters,
        }
    })
}

fn encode_openai_tool_choice(choice: &ToolChoice) -> Value {
    match choice {
        ToolChoice::Auto => Value::String("auto".to_string()),
        ToolChoice::None => Value::String("none".to_string()),
        ToolChoice::Required => Value::String("required".to_string()),
        ToolChoice::Tool { name } => json!({
            "type": "function",
            "function": { "name": name }
        }),
    }
}

fn encode_openai_message(msg: &Message, out: &mut Vec<OpenAIMessage>) {
    match msg.role {
        Role::System => {
            let text = msg.text();
            if !text.is_empty() {
                out.push(OpenAIMessage {
                    role: "system".to_string(),
                    content: Some(Value::String(text)),
                    tool_calls: None,
                    tool_call_id: None,
                    reasoning_content: None,
                });
            }
        }
        Role::User => {
            let has_image = msg
                .content
                .iter()
                .any(|p| matches!(p, ContentPart::Image { .. }));
            if !has_image {
                out.push(OpenAIMessage {
                    role: "user".to_string(),
                    content: Some(Value::String(msg.text())),
                    tool_calls: None,
                    tool_call_id: None,
                    reasoning_content: None,
                });
            } else {
                let mut parts = Vec::new();
                for part in &msg.content {
                    match part {
                        ContentPart::Text { text } => {
                            parts.push(json!({ "type": "text", "text": text }));
                        }
                        ContentPart::Image { source } => {
                            let url = match source {
                                ImageSource::Url { url } => url.clone(),
                                ImageSource::Base64 { media_type, data } => {
                                    format!("data:{media_type};base64,{data}")
                                }
                            };
                            parts.push(json!({
                                "type": "image_url",
                                "image_url": { "url": url }
                            }));
                        }
                        _ => {}
                    }
                }
                out.push(OpenAIMessage {
                    role: "user".to_string(),
                    content: Some(Value::Array(parts)),
                    tool_calls: None,
                    tool_call_id: None,
                    reasoning_content: None,
                });
            }
        }
        Role::Assistant => {
            let mut text = String::new();
            let mut thinking = String::new();
            let mut tool_calls = Vec::new();

            for part in &msg.content {
                match part {
                    ContentPart::Text { text: t } => text.push_str(t),
                    ContentPart::Thinking { thinking: t, .. } => thinking.push_str(t),
                    ContentPart::ToolCall(call) => {
                        tool_calls.push(OpenAIToolCallWire {
                            id: call.id.clone(),
                            r#type: "function".to_string(),
                            function: OpenAIFunctionCallWire {
                                name: call.name.clone(),
                                arguments: call.arguments_json_string(),
                            },
                        });
                    }
                    _ => {}
                }
            }

            let content = if text.is_empty() && !tool_calls.is_empty() {
                None
            } else {
                Some(Value::String(text))
            };

            out.push(OpenAIMessage {
                role: "assistant".to_string(),
                content,
                tool_calls: if tool_calls.is_empty() {
                    None
                } else {
                    Some(tool_calls)
                },
                tool_call_id: None,
                reasoning_content: if thinking.is_empty() {
                    None
                } else {
                    Some(thinking)
                },
            });
        }
        Role::Tool => {
            for part in &msg.content {
                if let ContentPart::ToolResult(result) = part {
                    out.push(OpenAIMessage {
                        role: "tool".to_string(),
                        content: Some(Value::String(result.content.clone())),
                        tool_calls: None,
                        tool_call_id: Some(result.tool_call_id.clone()),
                        reasoning_content: None,
                    });
                }
            }
        }
    }
}

fn map_openai_finish_reason(reason: &str) -> StopReason {
    match reason {
        "stop" => StopReason::EndTurn,
        "length" => StopReason::MaxTokens,
        "tool_calls" | "function_call" => StopReason::ToolUse,
        "content_filter" => StopReason::ContentFilter,
        other => StopReason::Other(other.to_string()),
    }
}

fn map_openai_usage(u: OpenAIUsage) -> Usage {
    Usage {
        input_tokens: u.prompt_tokens,
        output_tokens: u.completion_tokens,
        total_tokens: u.total_tokens,
        reasoning_tokens: u.completion_tokens_details.and_then(|d| d.reasoning_tokens),
        cache_read_tokens: u.prompt_tokens_details.and_then(|d| d.cached_tokens),
        cache_creation_tokens: None,
    }
}

fn parse_chat_response(resp: OpenAIChatResponse) -> Result<ChatResponse, Error> {
    let Some(choice) = resp.choices.into_iter().next() else {
        return Err(Error::api("OpenAI response contained no choices"));
    };

    let mut tool_calls = Vec::with_capacity(choice.message.tool_calls.len());
    for tc in choice.message.tool_calls {
        let arguments = parse_tool_arguments(&tc.function.arguments)?;
        tool_calls.push(ToolCall {
            id: tc.id,
            name: tc.function.name,
            arguments,
        });
    }

    let thinking = choice
        .message
        .reasoning_content
        .or(choice.message.reasoning)
        .filter(|s| !s.is_empty());

    let stop_reason = choice
        .finish_reason
        .as_deref()
        .map(map_openai_finish_reason);

    Ok(ChatResponse {
        id: resp.id,
        model: resp.model,
        content: choice.message.content.unwrap_or_default(),
        thinking,
        thinking_signature: None,
        tool_calls,
        stop_reason,
        usage: resp.usage.map(map_openai_usage),
    })
}

struct OpenAIStreamState<S> {
    sse: S,
    queue: VecDeque<Result<StreamEvent, Error>>,
    started: bool,
    open_tool_calls: BTreeSet<usize>,
    finish_reason: Option<StopReason>,
    done: bool,
}

fn openai_sse_to_events<S>(
    sse: S,
) -> impl Stream<Item = Result<StreamEvent, Error>> + Send + 'static
where
    S: Stream<Item = Result<crate::sse::SseEvent, Error>> + Send + Unpin + 'static,
{
    let state = OpenAIStreamState {
        sse,
        queue: VecDeque::new(),
        started: false,
        open_tool_calls: BTreeSet::new(),
        finish_reason: None,
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
                        flush_openai_end(&mut st);
                        continue;
                    }

                    // Check for in-stream error object before decoding chunk
                    let raw: Value = match serde_json::from_str(data) {
                        Ok(v) => v,
                        Err(e) => {
                            st.done = true;
                            return Some((Err(Error::Serialization(e)), st));
                        }
                    };

                    if raw.get("error").is_some() {
                        let msg = extract_error_message_from_value(&raw)
                            .unwrap_or_else(|| "OpenAI stream error".to_string());
                        st.done = true;
                        return Some((Err(Error::api(msg)), st));
                    }

                    let chunk: OpenAIStreamChunk = match serde_json::from_value(raw) {
                        Ok(c) => c,
                        Err(e) => {
                            st.done = true;
                            return Some((Err(Error::Serialization(e)), st));
                        }
                    };

                    if !st.started && (chunk.id.is_some() || chunk.model.is_some()) {
                        st.started = true;
                        st.queue.push_back(Ok(StreamEvent::MessageStart {
                            id: chunk.id,
                            model: chunk.model,
                        }));
                    }

                    if let Some(choice) = chunk.choices.into_iter().next() {
                        if let Some(thinking) = choice
                            .delta
                            .reasoning_content
                            .or(choice.delta.reasoning)
                            .or(choice.delta.thought)
                            && !thinking.is_empty()
                        {
                            st.queue.push_back(Ok(StreamEvent::ThinkingDelta(thinking)));
                        }

                        if let Some(content) = choice.delta.content
                            && !content.is_empty()
                        {
                            st.queue.push_back(Ok(StreamEvent::TextDelta(content)));
                        }

                        for tc in choice.delta.tool_calls {
                            let idx = tc.index;
                            if st.open_tool_calls.insert(idx) {
                                let name = tc
                                    .function
                                    .as_ref()
                                    .and_then(|f| f.name.clone())
                                    .unwrap_or_default();
                                st.queue.push_back(Ok(StreamEvent::ToolCallStart {
                                    index: idx,
                                    id: tc.id.unwrap_or_default(),
                                    name,
                                }));
                            }
                            if let Some(args) = tc.function.and_then(|f| f.arguments)
                                && !args.is_empty()
                            {
                                st.queue.push_back(Ok(StreamEvent::ToolCallDelta {
                                    index: idx,
                                    arguments_delta: args,
                                }));
                            }
                        }

                        if let Some(reason) = choice.finish_reason {
                            st.finish_reason = Some(map_openai_finish_reason(&reason));
                        }
                    }

                    if let Some(usage) = chunk.usage {
                        st.queue
                            .push_back(Ok(StreamEvent::Usage(map_openai_usage(usage))));
                    }
                }
                Some(Err(err)) => {
                    st.done = true;
                    return Some((Err(err), st));
                }
                None => {
                    flush_openai_end(&mut st);
                }
            }
        }
    })
}

fn flush_openai_end<S>(st: &mut OpenAIStreamState<S>) {
    st.done = true;
    let open = std::mem::take(&mut st.open_tool_calls);
    for idx in open {
        st.queue.push_back(Ok(StreamEvent::ToolCallEnd { index: idx }));
    }
    st.queue.push_back(Ok(StreamEvent::Done {
        stop_reason: st.finish_reason.take(),
    }));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::collect_stream;
    use mockito::Server;

    #[test]
    fn test_urls_normalization() {
        let client = OpenAIClient::new("k", "gpt-4o");
        assert_eq!(
            client.chat_completions_url(),
            "https://api.openai.com/v1/chat/completions"
        );
        assert_eq!(
            openai_models_url("https://api.openai.com/v1/"),
            "https://api.openai.com/v1/models"
        );
        assert_eq!(
            openai_models_url("https://example.com/v1/chat/completions"),
            "https://example.com/v1/models"
        );
    }

    #[tokio::test]
    async fn test_openai_chat_with_tools_and_thinking() {
        let mut server = Server::new_async().await;
        let mock = server
            .mock("POST", "/chat/completions")
            .match_header("authorization", "Bearer sk-test")
            .match_header("x-custom", "yes")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                r#"{
                    "id": "chatcmpl-1",
                    "model": "gpt-4o",
                    "choices": [{
                        "index": 0,
                        "message": {
                            "role": "assistant",
                            "content": null,
                            "reasoning_content": "Need weather data",
                            "tool_calls": [{
                                "id": "call_abc",
                                "type": "function",
                                "function": {
                                    "name": "get_weather",
                                    "arguments": "{\"city\":\"Tokyo\"}"
                                }
                            }]
                        },
                        "finish_reason": "tool_calls"
                    }],
                    "usage": {
                        "prompt_tokens": 25,
                        "completion_tokens": 15,
                        "total_tokens": 40,
                        "prompt_tokens_details": { "cached_tokens": 5 },
                        "completion_tokens_details": { "reasoning_tokens": 8 }
                    }
                }"#,
            )
            .create_async()
            .await;

        let client = OpenAIClient::new("sk-test", "gpt-4o")
            .with_base_url(server.url())
            .with_header("x-custom", "yes")
            .unwrap();

        let req = ChatRequest::new(vec![Message::user("Weather in Tokyo?")])
            .with_system("You are a weather assistant.")
            .with_tools(vec![ToolDefinition::new(
                "get_weather",
                "Get weather by city",
                json!({"type": "object", "properties": {"city": {"type": "string"}}}),
            )])
            .with_tool_choice(ToolChoice::Auto)
            .with_thinking(crate::ThinkingConfig::with_effort(ReasoningEffort::Medium));

        let resp = client.chat(req).await.unwrap();
        mock.assert_async().await;

        assert_eq!(resp.id.as_deref(), Some("chatcmpl-1"));
        assert_eq!(resp.model.as_deref(), Some("gpt-4o"));
        assert_eq!(resp.content, "");
        assert_eq!(resp.thinking.as_deref(), Some("Need weather data"));
        assert_eq!(resp.stop_reason, Some(StopReason::ToolUse));
        assert_eq!(resp.tool_calls.len(), 1);
        assert_eq!(resp.tool_calls[0].id, "call_abc");
        assert_eq!(resp.tool_calls[0].name, "get_weather");
        assert_eq!(resp.tool_calls[0].arguments, json!({"city": "Tokyo"}));

        let usage = resp.usage.unwrap();
        assert_eq!(usage.input_tokens, 25);
        assert_eq!(usage.output_tokens, 15);
        assert_eq!(usage.total_tokens, Some(40));
        assert_eq!(usage.cache_read_tokens, Some(5));
        assert_eq!(usage.reasoning_tokens, Some(8));
    }

    #[tokio::test]
    async fn test_openai_stream_text_thinking_and_tool_calls() {
        let mut server = Server::new_async().await;
        let sse_body = concat!(
            "data: {\"id\":\"chatcmpl-s1\",\"model\":\"o3-mini\",\"choices\":[{\"delta\":{\"reasoning_content\":\"Think \"}}]}\n\n",
            "data: {\"id\":\"chatcmpl-s1\",\"model\":\"o3-mini\",\"choices\":[{\"delta\":{\"reasoning_content\":\"more\"}}]}\n\n",
            "data: {\"id\":\"chatcmpl-s1\",\"model\":\"o3-mini\",\"choices\":[{\"delta\":{\"content\":\"Checking\"}}]}\n\n",
            "data: {\"id\":\"chatcmpl-s1\",\"model\":\"o3-mini\",\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"call_1\",\"type\":\"function\",\"function\":{\"name\":\"search\",\"arguments\":\"{\\\"q\\\":\"}}]}}]}\n\n",
            "data: {\"id\":\"chatcmpl-s1\",\"model\":\"o3-mini\",\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"function\":{\"arguments\":\"\\\"rust\\\"}\"}}]}}]}\n\n",
            "data: {\"id\":\"chatcmpl-s1\",\"model\":\"o3-mini\",\"choices\":[{\"delta\":{},\"finish_reason\":\"tool_calls\"}]}\n\n",
            "data: {\"id\":\"chatcmpl-s1\",\"model\":\"o3-mini\",\"choices\":[],\"usage\":{\"prompt_tokens\":12,\"completion_tokens\":18,\"total_tokens\":30}}\n\n",
            "data: [DONE]\n\n"
        );

        let mock = server
            .mock("POST", "/chat/completions")
            .match_header("accept", "text/event-stream")
            .with_status(200)
            .with_header("content-type", "text/event-stream")
            .with_body(sse_body)
            .create_async()
            .await;

        let client = OpenAIClient::new("sk-test", "o3-mini").with_base_url(server.url());
        let stream = client
            .chat_stream(ChatRequest::new(vec![Message::user("Search rust")]))
            .await
            .unwrap();
        let resp = collect_stream(stream).await.unwrap();
        mock.assert_async().await;

        assert_eq!(resp.id.as_deref(), Some("chatcmpl-s1"));
        assert_eq!(resp.thinking.as_deref(), Some("Think more"));
        assert_eq!(resp.content, "Checking");
        assert_eq!(resp.tool_calls.len(), 1);
        assert_eq!(resp.tool_calls[0].id, "call_1");
        assert_eq!(resp.tool_calls[0].name, "search");
        assert_eq!(resp.tool_calls[0].arguments, json!({"q": "rust"}));
        assert_eq!(resp.stop_reason, Some(StopReason::ToolUse));
        assert_eq!(resp.usage.unwrap().total_tokens, Some(30));
    }

    #[tokio::test]
    async fn test_openai_http_error_and_list_models() {
        let mut server = Server::new_async().await;
        let err_mock = server
            .mock("POST", "/chat/completions")
            .with_status(401)
            .with_header("content-type", "application/json")
            .with_body(r#"{"error":{"message":"Incorrect API key","type":"invalid_request_error"}}"#)
            .create_async()
            .await;

        let models_mock = server
            .mock("GET", "/models")
            .match_header("authorization", "Bearer sk-bad")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"data":[{"id":"gpt-4o"},{"id":"o3-mini"}]}"#)
            .create_async()
            .await;

        let client = OpenAIClient::new("sk-bad", "gpt-4o").with_base_url(server.url());
        let err = client
            .chat(ChatRequest::new(vec![Message::user("hi")]))
            .await
            .unwrap_err();
        assert_eq!(err.status_code(), Some(401));
        assert!(err.to_string().contains("Incorrect API key"));
        err_mock.assert_async().await;

        let models = client.list_models().await.unwrap();
        assert_eq!(models, vec!["gpt-4o", "o3-mini"]);
        models_mock.assert_async().await;
    }

    #[test]
    fn test_openai_multimodal_and_tool_result_encoding() {
        let client = OpenAIClient::new("k", "gpt-4o");
        let req = ChatRequest::new(vec![
            Message::user_parts(vec![
                ContentPart::text("Describe this image"),
                ContentPart::image_base64("image/png", "AAAA"),
            ]),
            Message::assistant_parts(vec![ContentPart::ToolCall(ToolCall::new(
                "call_1",
                "inspect",
                json!({"x": 1}),
            ))]),
            Message::tool_result("call_1", Some("inspect"), "ok", false),
        ]);

        let body = serde_json::to_value(client.build_request_body(&req, false).unwrap()).unwrap();
        assert_eq!(body["messages"][0]["content"][1]["image_url"]["url"], "data:image/png;base64,AAAA");
        assert_eq!(body["messages"][1]["tool_calls"][0]["function"]["name"], "inspect");
        assert_eq!(body["messages"][2]["role"], "tool");
        assert_eq!(body["messages"][2]["tool_call_id"], "call_1");
    }
}
