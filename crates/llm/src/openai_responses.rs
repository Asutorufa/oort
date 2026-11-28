use std::collections::{BTreeSet, VecDeque};

use futures_util::{Stream, StreamExt};
use reqwest::Client as HttpClient;
use reqwest::header::HeaderMap;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::Error;
use crate::client::{ChatStream, LlmClient};
use crate::error::extract_error_message_from_value;
use crate::http::{apply_headers, bytes_stream, ensure_success, insert_header, normalize_base_url};
use crate::openai::{
    DEFAULT_OPENAI_BASE_URL, extract_openai_cache_fields, fetch_openai_models, reasoning_effort_str,
};
use crate::sse::parse_sse_stream;
use crate::types::{
    ChatRequest, ChatResponse, ContentPart, DocumentSource, ImageSource, Message, Role, StopReason,
    StreamEvent, ToolCall, ToolChoice, ToolDefinition, Usage, parse_tool_arguments,
};

/// HTTP client for the OpenAI Responses API (`/v1/responses`).
#[derive(Debug, Clone)]
pub struct OpenAIResponsesClient {
    http: HttpClient,
    base_url: String,
    api_key: String,
    model: String,
    extra_headers: HeaderMap,
}

impl OpenAIResponsesClient {
    pub fn new(api_key: impl Into<String>, model: impl Into<String>) -> Self {
        Self {
            http: HttpClient::new(),
            base_url: DEFAULT_OPENAI_BASE_URL.to_string(),
            api_key: api_key.into(),
            model: model.into(),
            extra_headers: HeaderMap::new(),
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

    pub fn model(&self) -> &str {
        &self.model
    }

    pub fn set_model(&mut self, model: impl Into<String>) {
        self.model = model.into();
    }

    fn responses_url(&self) -> String {
        let base = normalize_base_url(&self.base_url, DEFAULT_OPENAI_BASE_URL);
        if base.ends_with("/responses") {
            base.to_string()
        } else {
            format!("{base}/responses")
        }
    }

    pub(crate) fn build_request_body(
        &self,
        request: &ChatRequest,
        stream: bool,
    ) -> Result<ResponsesApiRequest, Error> {
        let model = request.resolve_model(&self.model)?.to_string();
        let (instructions, non_system) = request.split_system_messages();

        let mut input = Vec::new();
        for msg in non_system {
            encode_responses_input(msg, &mut input);
        }

        let tools = if request.tools.is_empty() {
            None
        } else {
            Some(request.tools.iter().map(encode_responses_tool).collect())
        };

        let tool_choice = if tools.is_some() {
            request
                .tool_choice
                .as_ref()
                .map(encode_responses_tool_choice)
        } else {
            None
        };

        let reasoning =
            request
                .thinking
                .as_ref()
                .filter(|t| t.enabled)
                .map(|t| ResponsesReasoningConfig {
                    effort: t.effort.map(reasoning_effort_str),
                    summary: Some("auto"),
                });

        let (prompt_cache_key, prompt_cache_retention) =
            extract_openai_cache_fields(request.prompt_cache.as_ref());

        Ok(ResponsesApiRequest {
            model,
            instructions,
            input,
            tools,
            tool_choice,
            max_output_tokens: request.max_tokens,
            // Reasoning models reject sampling parameters.
            temperature: request.temperature.filter(|_| reasoning.is_none()),
            top_p: request.top_p.filter(|_| reasoning.is_none()),
            reasoning,
            prompt_cache_key,
            prompt_cache_retention,
            stream: Some(stream),
        })
    }
}

impl LlmClient for OpenAIResponsesClient {
    async fn chat(&self, request: ChatRequest) -> Result<ChatResponse, Error> {
        let body = self.build_request_body(&request, false)?;

        let mut req_builder = self.http.post(self.responses_url()).json(&body);
        if !self.api_key.trim().is_empty() {
            req_builder = req_builder.bearer_auth(&self.api_key);
        }
        req_builder = apply_headers(req_builder, &self.extra_headers);

        let resp = ensure_success(req_builder.send().await?).await?;
        let api_resp: ResponsesApiResponse = resp.json().await?;
        parse_responses_body(api_resp)
    }

    async fn chat_stream(&self, request: ChatRequest) -> Result<ChatStream, Error> {
        let body = self.build_request_body(&request, true)?;

        let mut req_builder = self
            .http
            .post(self.responses_url())
            .header("Accept", "text/event-stream")
            .json(&body);
        if !self.api_key.trim().is_empty() {
            req_builder = req_builder.bearer_auth(&self.api_key);
        }
        req_builder = apply_headers(req_builder, &self.extra_headers);

        let resp = ensure_success(req_builder.send().await?).await?;
        let sse = parse_sse_stream(bytes_stream(resp));
        Ok(Box::pin(responses_sse_to_events(sse)))
    }

    async fn list_models(&self) -> Result<Vec<String>, Error> {
        fetch_openai_models(
            &self.http,
            &self.base_url,
            &self.api_key,
            &self.extra_headers,
        )
        .await
    }
}

// --- Wire types ---

#[derive(Debug, Serialize)]
pub(crate) struct ResponsesApiRequest {
    model: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    instructions: Option<String>,
    input: Vec<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<Vec<Value>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_choice: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_output_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    top_p: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    reasoning: Option<ResponsesReasoningConfig>,
    #[serde(skip_serializing_if = "Option::is_none")]
    prompt_cache_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    prompt_cache_retention: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    stream: Option<bool>,
}

#[derive(Debug, Serialize)]
struct ResponsesReasoningConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    effort: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    summary: Option<&'static str>,
}

#[derive(Debug, Deserialize)]
struct ResponsesApiResponse {
    id: Option<String>,
    model: Option<String>,
    status: Option<String>,
    error: Option<Value>,
    incomplete_details: Option<ResponsesIncompleteDetails>,
    #[serde(default)]
    output: Vec<ResponsesOutputItem>,
    usage: Option<ResponsesUsage>,
}

#[derive(Debug, Deserialize)]
struct ResponsesIncompleteDetails {
    reason: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ResponsesOutputItem {
    #[serde(rename = "type", default)]
    item_type: String,
    #[serde(default)]
    content: Vec<ResponsesContentPart>,
    #[serde(default)]
    summary: Vec<ResponsesContentPart>,
    call_id: Option<String>,
    id: Option<String>,
    name: Option<String>,
    arguments: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ResponsesContentPart {
    #[serde(rename = "type", default)]
    part_type: String,
    #[serde(default)]
    text: String,
}

#[derive(Debug, Deserialize)]
struct ResponsesUsage {
    #[serde(default)]
    input_tokens: u64,
    #[serde(default)]
    output_tokens: u64,
    total_tokens: Option<u64>,
    input_tokens_details: Option<ResponsesInputTokensDetails>,
    output_tokens_details: Option<ResponsesOutputTokensDetails>,
}

#[derive(Debug, Deserialize)]
struct ResponsesInputTokensDetails {
    cached_tokens: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct ResponsesOutputTokensDetails {
    reasoning_tokens: Option<u64>,
}

// --- Helpers ---

fn encode_responses_tool(tool: &ToolDefinition) -> Value {
    json!({
        "type": "function",
        "name": tool.name,
        "description": tool.description,
        "parameters": tool.parameters,
    })
}

fn encode_responses_tool_choice(choice: &ToolChoice) -> Value {
    match choice {
        ToolChoice::Auto => Value::String("auto".to_string()),
        ToolChoice::None => Value::String("none".to_string()),
        ToolChoice::Required => Value::String("required".to_string()),
        ToolChoice::Tool { name } => json!({
            "type": "function",
            "name": name
        }),
    }
}

fn encode_responses_input(msg: &Message, out: &mut Vec<Value>) {
    match msg.role {
        Role::System => {
            let text = msg.text();
            if !text.is_empty() {
                out.push(json!({
                    "type": "message",
                    "role": "system",
                    "content": [{ "type": "input_text", "text": text }]
                }));
            }
        }
        Role::User => {
            let mut content = Vec::new();
            for part in &msg.content {
                match part {
                    ContentPart::Text { text } => {
                        content.push(json!({ "type": "input_text", "text": text }));
                    }
                    ContentPart::Image { source } => {
                        let url = match source {
                            ImageSource::Url { url } => url.clone(),
                            ImageSource::Base64 { media_type, data } => {
                                format!("data:{media_type};base64,{data}")
                            }
                        };
                        content.push(json!({
                            "type": "input_image",
                            "image_url": url
                        }));
                    }
                    ContentPart::Document { source, filename } => match source {
                        DocumentSource::Base64 { media_type, data } => {
                            let name = filename.clone().unwrap_or_else(|| {
                                if media_type == "application/pdf" {
                                    "document.pdf".to_string()
                                } else {
                                    "document".to_string()
                                }
                            });
                            content.push(json!({
                                "type": "input_file",
                                "filename": name,
                                "file_data": format!("data:{media_type};base64,{data}")
                            }));
                        }
                        DocumentSource::Url { url, .. } => {
                            content.push(json!({
                                "type": "input_file",
                                "file_url": url
                            }));
                        }
                        DocumentSource::FileId { file_id, .. } => {
                            content.push(json!({
                                "type": "input_file",
                                "file_id": file_id
                            }));
                        }
                    },
                    _ => {}
                }
            }
            if !content.is_empty() {
                out.push(json!({
                    "type": "message",
                    "role": "user",
                    "content": content
                }));
            }
        }
        Role::Assistant => {
            let mut text_parts = Vec::new();
            let mut tool_items = Vec::new();
            for part in &msg.content {
                match part {
                    ContentPart::Text { text } if !text.is_empty() => {
                        text_parts.push(json!({ "type": "output_text", "text": text }));
                    }
                    ContentPart::ToolCall(call) => {
                        tool_items.push(json!({
                            "type": "function_call",
                            "call_id": call.id,
                            "name": call.name,
                            "arguments": call.arguments_json_string(),
                        }));
                    }
                    _ => {}
                }
            }
            if !text_parts.is_empty() {
                out.push(json!({
                    "type": "message",
                    "role": "assistant",
                    "content": text_parts
                }));
            }
            out.extend(tool_items);
        }
        Role::Tool => {
            for part in &msg.content {
                if let ContentPart::ToolResult(result) = part {
                    out.push(json!({
                        "type": "function_call_output",
                        "call_id": result.tool_call_id,
                        "output": result.content,
                    }));
                }
            }
        }
    }
}

fn map_responses_usage(u: ResponsesUsage) -> Usage {
    Usage {
        input_tokens: u.input_tokens,
        output_tokens: u.output_tokens,
        total_tokens: u.total_tokens,
        reasoning_tokens: u.output_tokens_details.and_then(|d| d.reasoning_tokens),
        cache_read_tokens: u.input_tokens_details.and_then(|d| d.cached_tokens),
        cache_creation_tokens: None,
    }
}

fn parse_responses_body(resp: ResponsesApiResponse) -> Result<ChatResponse, Error> {
    if resp.status.as_deref() == Some("failed") || resp.error.is_some() {
        let msg = resp
            .error
            .as_ref()
            .and_then(|e| {
                e.get("message")
                    .and_then(Value::as_str)
                    .or_else(|| e.as_str())
            })
            .unwrap_or("OpenAI Responses request failed");
        return Err(Error::api(msg));
    }

    let mut content = String::new();
    let mut thinking: Option<String> = None;
    let mut tool_calls = Vec::new();

    for item in resp.output {
        if item.item_type == "function_call" {
            let id = item.call_id.or(item.id).unwrap_or_default();
            let name = item.name.unwrap_or_default();
            let arguments = parse_tool_arguments(item.arguments.as_deref().unwrap_or(""))?;
            tool_calls.push(ToolCall {
                id,
                name,
                arguments,
            });
            continue;
        }

        for part in item.content {
            match part.part_type.as_str() {
                "output_text" | "text" => content.push_str(&part.text),
                "reasoning" | "summary_text" | "reasoning_text" => {
                    append_thinking(&mut thinking, &part.text);
                }
                _ => {}
            }
        }
        for part in item.summary {
            if matches!(part.part_type.as_str(), "summary_text" | "reasoning") {
                append_thinking(&mut thinking, &part.text);
            }
        }
    }

    let stop_reason = if !tool_calls.is_empty() {
        Some(StopReason::ToolUse)
    } else if resp.status.as_deref() == Some("incomplete") {
        match resp
            .incomplete_details
            .as_ref()
            .and_then(|d| d.reason.as_deref())
        {
            Some("max_output_tokens") => Some(StopReason::MaxTokens),
            Some("content_filter") => Some(StopReason::ContentFilter),
            Some(other) => Some(StopReason::Other(other.to_string())),
            None => Some(StopReason::Other("incomplete".to_string())),
        }
    } else {
        Some(StopReason::EndTurn)
    };

    Ok(ChatResponse {
        id: resp.id,
        model: resp.model,
        content,
        thinking,
        thinking_signature: None,
        tool_calls,
        stop_reason,
        usage: resp.usage.map(map_responses_usage),
    })
}

fn append_thinking(target: &mut Option<String>, value: &str) {
    if value.is_empty() {
        return;
    }
    target.get_or_insert_with(String::new).push_str(value);
}

struct ResponsesStreamState<S> {
    sse: S,
    queue: VecDeque<Result<StreamEvent, Error>>,
    open_tool_calls: BTreeSet<usize>,
    had_tool_calls: bool,
    done: bool,
}

fn responses_sse_to_events<S>(
    sse: S,
) -> impl Stream<Item = Result<StreamEvent, Error>> + Send + 'static
where
    S: Stream<Item = Result<crate::sse::SseEvent, Error>> + Send + Unpin + 'static,
{
    let state = ResponsesStreamState {
        sse,
        queue: VecDeque::new(),
        open_tool_calls: BTreeSet::new(),
        had_tool_calls: false,
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
                        finish_responses_stream(&mut st, None);
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
                        Some("response.created") => {
                            let id = value
                                .pointer("/response/id")
                                .and_then(Value::as_str)
                                .map(ToString::to_string);
                            let model = value
                                .pointer("/response/model")
                                .and_then(Value::as_str)
                                .map(ToString::to_string);
                            st.queue
                                .push_back(Ok(StreamEvent::MessageStart { id, model }));
                        }
                        Some("response.output_item.added") => {
                            if value.pointer("/item/type").and_then(Value::as_str)
                                == Some("function_call")
                            {
                                let idx = value
                                    .get("output_index")
                                    .and_then(Value::as_u64)
                                    .unwrap_or(0)
                                    as usize;
                                let id = value
                                    .pointer("/item/call_id")
                                    .or_else(|| value.pointer("/item/id"))
                                    .and_then(Value::as_str)
                                    .unwrap_or_default()
                                    .to_string();
                                let name = value
                                    .pointer("/item/name")
                                    .and_then(Value::as_str)
                                    .unwrap_or_default()
                                    .to_string();
                                st.open_tool_calls.insert(idx);
                                st.had_tool_calls = true;
                                st.queue.push_back(Ok(StreamEvent::ToolCallStart {
                                    index: idx,
                                    id,
                                    name,
                                }));
                            }
                        }
                        Some("response.output_text.delta") => {
                            if let Some(delta) = value.get("delta").and_then(Value::as_str)
                                && !delta.is_empty()
                            {
                                st.queue
                                    .push_back(Ok(StreamEvent::TextDelta(delta.to_string())));
                            }
                        }
                        Some(
                            "response.reasoning.delta"
                            | "response.reasoning_summary.delta"
                            | "response.reasoning_summary_text.delta"
                            | "response.reasoning_text.delta",
                        ) => {
                            if let Some(delta) = value
                                .get("delta")
                                .and_then(Value::as_str)
                                .or_else(|| value.get("text").and_then(Value::as_str))
                                && !delta.is_empty()
                            {
                                st.queue
                                    .push_back(Ok(StreamEvent::ThinkingDelta(delta.to_string())));
                            }
                        }
                        Some("response.function_call_arguments.delta") => {
                            let idx = value
                                .get("output_index")
                                .and_then(Value::as_u64)
                                .unwrap_or(0) as usize;
                            if let Some(delta) = value.get("delta").and_then(Value::as_str)
                                && !delta.is_empty()
                            {
                                st.queue.push_back(Ok(StreamEvent::ToolCallDelta {
                                    index: idx,
                                    arguments_delta: delta.to_string(),
                                }));
                            }
                        }
                        Some("response.output_item.done") => {
                            if value.pointer("/item/type").and_then(Value::as_str)
                                == Some("function_call")
                            {
                                let idx = value
                                    .get("output_index")
                                    .and_then(Value::as_u64)
                                    .unwrap_or(0)
                                    as usize;
                                if st.open_tool_calls.remove(&idx) {
                                    st.queue
                                        .push_back(Ok(StreamEvent::ToolCallEnd { index: idx }));
                                }
                            }
                        }
                        Some("response.completed" | "response.done") => {
                            if let Some(usage_val) = value.pointer("/response/usage")
                                && let Ok(usage) =
                                    serde_json::from_value::<ResponsesUsage>(usage_val.clone())
                            {
                                st.queue
                                    .push_back(Ok(StreamEvent::Usage(map_responses_usage(usage))));
                            }
                            let stop_reason = if st.had_tool_calls {
                                Some(StopReason::ToolUse)
                            } else {
                                Some(StopReason::EndTurn)
                            };
                            finish_responses_stream(&mut st, stop_reason);
                        }
                        Some("response.failed" | "response.incomplete" | "error") => {
                            let msg = extract_error_message_from_value(&value)
                                .unwrap_or_else(|| "OpenAI Responses stream error".to_string());
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
                    finish_responses_stream(&mut st, None);
                }
            }
        }
    })
}

fn finish_responses_stream<S>(st: &mut ResponsesStreamState<S>, stop_reason: Option<StopReason>) {
    st.done = true;
    let open = std::mem::take(&mut st.open_tool_calls);
    for idx in open {
        st.queue
            .push_back(Ok(StreamEvent::ToolCallEnd { index: idx }));
    }
    let reason = stop_reason.or(if st.had_tool_calls {
        Some(StopReason::ToolUse)
    } else {
        None
    });
    st.queue.push_back(Ok(StreamEvent::Done {
        stop_reason: reason,
    }));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::collect_stream;
    use mockito::Server;

    #[tokio::test]
    async fn test_responses_chat_and_function_call() {
        let mut server = Server::new_async().await;
        let mock = server
            .mock("POST", "/responses")
            .match_header("authorization", "Bearer sk-resp")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                r#"{
                    "id": "resp_1",
                    "model": "gpt-4.1",
                    "status": "completed",
                    "output": [
                        {
                            "type": "reasoning",
                            "summary": [{"type": "summary_text", "text": "Checking tool"}]
                        },
                        {
                            "type": "message",
                            "content": [{"type": "output_text", "text": "Let me look that up."}]
                        },
                        {
                            "type": "function_call",
                            "call_id": "call_99",
                            "name": "lookup",
                            "arguments": "{\"key\":\"oort\"}"
                        }
                    ],
                    "usage": {
                        "input_tokens": 14,
                        "output_tokens": 22,
                        "total_tokens": 36,
                        "output_tokens_details": { "reasoning_tokens": 6 }
                    }
                }"#,
            )
            .create_async()
            .await;

        let client = OpenAIResponsesClient::new("sk-resp", "gpt-4.1").with_base_url(server.url());
        let resp = client
            .chat(ChatRequest::new(vec![Message::user("lookup oort")]))
            .await
            .unwrap();
        mock.assert_async().await;

        assert_eq!(resp.id.as_deref(), Some("resp_1"));
        assert_eq!(resp.thinking.as_deref(), Some("Checking tool"));
        assert_eq!(resp.content, "Let me look that up.");
        assert_eq!(resp.tool_calls.len(), 1);
        assert_eq!(resp.tool_calls[0].id, "call_99");
        assert_eq!(resp.tool_calls[0].arguments, json!({"key": "oort"}));
        assert_eq!(resp.stop_reason, Some(StopReason::ToolUse));
        assert_eq!(resp.usage.as_ref().unwrap().reasoning_tokens, Some(6));
    }

    #[tokio::test]
    async fn test_responses_stream_with_tool_call_and_usage() {
        let mut server = Server::new_async().await;
        let body = concat!(
            "event: response.created\n",
            "data: {\"type\":\"response.created\",\"response\":{\"id\":\"resp_s1\",\"model\":\"gpt-4.1\"}}\n\n",
            "event: response.reasoning_summary_text.delta\n",
            "data: {\"type\":\"response.reasoning_summary_text.delta\",\"delta\":\"Thinking...\"}\n\n",
            "event: response.output_text.delta\n",
            "data: {\"type\":\"response.output_text.delta\",\"delta\":\"Hello \"}\n\n",
            "event: response.output_text.delta\n",
            "data: {\"type\":\"response.output_text.delta\",\"delta\":\"world\"}\n\n",
            "event: response.output_item.added\n",
            "data: {\"type\":\"response.output_item.added\",\"output_index\":1,\"item\":{\"type\":\"function_call\",\"call_id\":\"call_fn1\",\"name\":\"calc\"}}\n\n",
            "event: response.function_call_arguments.delta\n",
            "data: {\"type\":\"response.function_call_arguments.delta\",\"output_index\":1,\"delta\":\"{\\\"a\\\":1\"}\n\n",
            "event: response.function_call_arguments.delta\n",
            "data: {\"type\":\"response.function_call_arguments.delta\",\"output_index\":1,\"delta\":\",\\\"b\\\":2}\"}\n\n",
            "event: response.output_item.done\n",
            "data: {\"type\":\"response.output_item.done\",\"output_index\":1,\"item\":{\"type\":\"function_call\"}}\n\n",
            "event: response.completed\n",
            "data: {\"type\":\"response.completed\",\"response\":{\"usage\":{\"input_tokens\":10,\"output_tokens\":20,\"total_tokens\":30}}}\n\n"
        );

        let mock = server
            .mock("POST", "/responses")
            .match_header("accept", "text/event-stream")
            .with_status(200)
            .with_header("content-type", "text/event-stream")
            .with_body(body)
            .create_async()
            .await;

        let client = OpenAIResponsesClient::new("sk-resp", "gpt-4.1").with_base_url(server.url());
        let stream = client
            .chat_stream(ChatRequest::new(vec![Message::user("1+2?")]))
            .await
            .unwrap();
        let resp = collect_stream(stream).await.unwrap();
        mock.assert_async().await;

        assert_eq!(resp.id.as_deref(), Some("resp_s1"));
        assert_eq!(resp.thinking.as_deref(), Some("Thinking..."));
        assert_eq!(resp.content, "Hello world");
        assert_eq!(resp.tool_calls.len(), 1);
        assert_eq!(resp.tool_calls[0].id, "call_fn1");
        assert_eq!(resp.tool_calls[0].name, "calc");
        assert_eq!(resp.tool_calls[0].arguments, json!({"a": 1, "b": 2}));
        assert_eq!(resp.stop_reason, Some(StopReason::ToolUse));
        assert_eq!(resp.usage.unwrap().total_tokens, Some(30));
    }

    #[tokio::test]
    async fn test_responses_stream_error_event() {
        let mut server = Server::new_async().await;
        let body = "event: response.failed\ndata: {\"type\":\"response.failed\",\"response\":{\"error\":{\"message\":\"Rate limit exceeded\"}}}\n\n";
        let mock = server
            .mock("POST", "/responses")
            .with_status(200)
            .with_header("content-type", "text/event-stream")
            .with_body(body)
            .create_async()
            .await;

        let client = OpenAIResponsesClient::new("sk-resp", "gpt-4.1").with_base_url(server.url());
        let stream = client
            .chat_stream(ChatRequest::new(vec![Message::user("hi")]))
            .await
            .unwrap();
        let err = collect_stream(stream).await.unwrap_err();
        mock.assert_async().await;
        assert!(err.to_string().contains("Rate limit exceeded"));
    }
}
