use std::collections::{BTreeMap, HashSet, VecDeque};

use futures_util::{Stream, StreamExt};
use reqwest::Client as HttpClient;
use reqwest::header::HeaderMap;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::Error;
use crate::client::{ChatStream, LlmClient};
use crate::error::extract_error_message_from_value;
use crate::http::{apply_headers, bytes_stream, ensure_success, insert_header, normalize_base_url};
use crate::openai_responses::merge_extra_body;
use crate::sse::parse_sse_stream;
use crate::types::{
    ChatRequest, ChatResponse, Citation, ContentPart, DocumentSource, ImageSource, Message,
    ReasoningEffort, Role, StopReason, StreamEvent, ToolCall, ToolChoice, ToolDefinition, Usage,
    Vendor, extract_domain,
};

pub const DEFAULT_GEMINI_BASE_URL: &str = "https://generativelanguage.googleapis.com/v1beta";

/// HTTP client for Google Gemini API (AI Studio) and Vertex AI Gemini endpoints.
#[derive(Debug, Clone)]
pub struct GeminiClient {
    http: HttpClient,
    base_url: String,
    api_key: Option<String>,
    bearer_token: Option<String>,
    project_id: Option<String>,
    location: Option<String>,
    model: String,
    vendor: Vendor,
    extra_body: Option<Value>,
    google_search: bool,
    extra_headers: HeaderMap,
}

impl GeminiClient {
    /// Creates a Google AI Studio Gemini client authenticated via `x-goog-api-key`.
    pub fn new(api_key: impl Into<String>, model: impl Into<String>) -> Self {
        Self {
            http: HttpClient::new(),
            base_url: DEFAULT_GEMINI_BASE_URL.to_string(),
            api_key: Some(api_key.into()),
            bearer_token: None,
            project_id: None,
            location: None,
            model: model.into(),
            vendor: Vendor::Gemini,
            extra_body: None,
            google_search: false,
            extra_headers: HeaderMap::new(),
        }
    }

    /// Creates a Vertex AI Gemini client authenticated via OAuth bearer token.
    pub fn new_vertex_ai(
        project_id: impl Into<String>,
        location: impl Into<String>,
        model: impl Into<String>,
        bearer_token: impl Into<String>,
    ) -> Self {
        let location = location.into();
        let base_url = format!("https://{location}-aiplatform.googleapis.com/v1");
        Self {
            http: HttpClient::new(),
            base_url,
            api_key: None,
            bearer_token: Some(bearer_token.into()),
            project_id: Some(project_id.into()),
            location: Some(location),
            model: model.into(),
            vendor: Vendor::Gemini,
            extra_body: None,
            google_search: false,
            extra_headers: HeaderMap::new(),
        }
    }

    pub fn with_vendor(mut self, vendor: Vendor) -> Self {
        self.vendor = vendor;
        self
    }

    pub fn with_extra_body(mut self, extra_body: Option<Value>) -> Self {
        self.extra_body = extra_body;
        self
    }

    pub fn with_base_url(mut self, base_url: impl Into<String>) -> Self {
        self.base_url = base_url.into();
        self
    }

    pub fn with_http_client(mut self, http: HttpClient) -> Self {
        self.http = http;
        self
    }

    pub fn with_google_search(mut self, enabled: bool) -> Self {
        self.google_search = enabled;
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

    fn action_url(&self, model: &str, action: &str) -> String {
        let base = normalize_base_url(&self.base_url, DEFAULT_GEMINI_BASE_URL);
        if let Some(project_id) = &self.project_id {
            let location = self.location.as_deref().unwrap_or("us-central1");
            format!(
                "{base}/projects/{project_id}/locations/{location}/publishers/google/models/{model}:{action}"
            )
        } else {
            format!("{base}/models/{model}:{action}")
        }
    }

    fn models_url(&self) -> String {
        let base = normalize_base_url(&self.base_url, DEFAULT_GEMINI_BASE_URL);
        if let Some(project_id) = &self.project_id {
            let location = self.location.as_deref().unwrap_or("us-central1");
            format!("{base}/projects/{project_id}/locations/{location}/publishers/google/models")
        } else {
            format!("{base}/models")
        }
    }

    fn apply_auth(&self, mut builder: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        if let Some(api_key) = &self.api_key
            && !api_key.trim().is_empty()
        {
            builder = builder.header("x-goog-api-key", api_key);
        }
        if let Some(token) = &self.bearer_token
            && !token.trim().is_empty()
        {
            builder = builder.bearer_auth(token);
        }
        apply_headers(builder, &self.extra_headers)
    }

    pub(crate) fn build_request_body(
        &self,
        request: &ChatRequest,
    ) -> Result<(String, GeminiGenerateContentRequest), Error> {
        let model = request.resolve_model(&self.model)?.to_string();
        let (system_text, non_system) = request.split_system_messages();

        // Track tool_call_id -> tool_name from assistant turns so ToolResult without explicit `name`
        // can still populate Gemini's required `functionResponse.name`.
        let mut call_names: BTreeMap<String, String> = BTreeMap::new();
        for msg in &non_system {
            for part in &msg.content {
                if let ContentPart::ToolCall(tc) = part {
                    call_names.insert(tc.id.clone(), tc.name.clone());
                }
            }
        }

        let mut contents: Vec<GeminiContent> = Vec::new();
        for msg in non_system {
            let (role, mut parts) = encode_gemini_message(msg, &call_names);
            if role == "model" {
                ensure_call_signature(&model, &mut parts);
            }
            if parts.is_empty() {
                continue;
            }
            if let Some(last) = contents.last_mut()
                && last.role == role
            {
                last.parts.extend(parts);
            } else {
                contents.push(GeminiContent { role, parts });
            }
        }

        let system_instruction = system_text.map(|text| GeminiContent {
            role: "user".to_string(),
            parts: vec![GeminiPart {
                text: Some(text),
                ..Default::default()
            }],
        });

        let mut tools: Vec<Value> = Vec::new();
        if !request.tools.is_empty() {
            let decls: Vec<GeminiFunctionDeclaration> = request
                .tools
                .iter()
                .map(encode_gemini_function_declaration)
                .collect();
            tools.push(json!({ "functionDeclarations": decls }));
        }
        let enable_search = self.google_search
            || request
                .server_tools
                .as_ref()
                .is_some_and(|st| st.web_search_enabled());
        if enable_search {
            tools.push(json!({ "google_search": {} }));
        }
        if request.server_tools.as_ref().is_some_and(|st| st.url_context) {
            tools.push(json!({ "url_context": {} }));
        }
        if request
            .server_tools
            .as_ref()
            .is_some_and(|st| st.code_execution)
        {
            tools.push(json!({ "code_execution": {} }));
        }

        let has_server_tools = enable_search
            || request.server_tools.as_ref().is_some_and(|st| st.url_context)
            || request
                .server_tools
                .as_ref()
                .is_some_and(|st| st.code_execution);

        let mut tool_config = if !request.tools.is_empty() {
            request.tool_choice.as_ref().map(encode_gemini_tool_choice)
        } else {
            None
        };
        if !request.tools.is_empty() && has_server_tools {
            tool_config
                .get_or_insert_with(GeminiToolConfig::default)
                .include_server_side_tool_invocations = Some(true);
        }

        let thinking_config = request
            .thinking
            .as_ref()
            .and_then(|t| gemini_thinking(&model, t));

        let generation_config = build_generation_config(request, thinking_config);

        let cached_content = request
            .prompt_cache
            .as_ref()
            .filter(|c| c.enabled)
            .and_then(|c| c.cached_content.as_deref())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(ToString::to_string);

        Ok((
            model,
            GeminiGenerateContentRequest {
                contents,
                system_instruction,
                tools: if tools.is_empty() { None } else { Some(tools) },
                tool_config,
                generation_config,
                cached_content,
            },
        ))
    }

    pub(crate) fn build_request_json(
        &self,
        request: &ChatRequest,
    ) -> Result<(String, Value), Error> {
        let (model, body) = self.build_request_body(request)?;
        let mut val = serde_json::to_value(body)?;
        merge_extra_body(&mut val, self.extra_body.as_ref());
        merge_extra_body(&mut val, request.extra_body.as_ref());
        Ok((model, val))
    }
}

impl LlmClient for GeminiClient {
    async fn chat(&self, request: ChatRequest) -> Result<ChatResponse, Error> {
        let (model, body) = self.build_request_json(&request)?;
        let url = self.action_url(&model, "generateContent");
        let req_builder = self.apply_auth(self.http.post(url).json(&body));

        let resp = ensure_success(req_builder.send().await?).await?;
        let api_resp: GeminiGenerateContentResponse = resp.json().await?;
        parse_gemini_response(api_resp)
    }

    async fn chat_stream(&self, request: ChatRequest) -> Result<ChatStream, Error> {
        let (model, body) = self.build_request_json(&request)?;
        let url = self.action_url(&model, "streamGenerateContent");
        let req_builder = self.apply_auth(
            self.http
                .post(url)
                .query(&[("alt", "sse")])
                .header("Accept", "text/event-stream")
                .json(&body),
        );

        let resp = ensure_success(req_builder.send().await?).await?;
        let sse = parse_sse_stream(bytes_stream(resp));
        Ok(Box::pin(gemini_sse_to_events(sse)))
    }

    async fn list_models(&self) -> Result<Vec<String>, Error> {
        let models_url = self.models_url();
        let mut page_token: Option<String> = None;
        let mut models = Vec::new();
        let mut seen = HashSet::new();

        loop {
            let mut query = vec![("pageSize", "1000")];
            if let Some(token) = &page_token {
                query.push(("pageToken", token.as_str()));
            }

            let req_builder = self.apply_auth(self.http.get(&models_url).query(&query));
            let resp = ensure_success(req_builder.send().await?).await?;
            let page: GeminiListModelsResponse = resp.json().await?;

            for model in page.models.into_iter().chain(page.publisher_models) {
                let supports_generate = (model.supported_generation_methods.is_empty()
                    && model.supported_actions.is_empty())
                    || model
                        .supported_generation_methods
                        .iter()
                        .chain(model.supported_actions.iter())
                        .any(|a| a == "generateContent");
                if !supports_generate {
                    continue;
                }

                let model_id = model
                    .base_model_id
                    .filter(|id| !id.trim().is_empty())
                    .unwrap_or_else(|| {
                        model
                            .name
                            .rsplit('/')
                            .next()
                            .unwrap_or_default()
                            .to_string()
                    });
                let model_id = model_id.strip_prefix("models/").unwrap_or(&model_id).trim();
                if !model_id.is_empty() && seen.insert(model_id.to_string()) {
                    models.push(model_id.to_string());
                }
            }

            let Some(next) = page.next_page_token.filter(|t| !t.is_empty()) else {
                break;
            };
            if page_token.as_deref() == Some(next.as_str()) {
                break;
            }
            page_token = Some(next);
        }

        Ok(models)
    }
}

// --- Wire types ---

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct GeminiGenerateContentRequest {
    contents: Vec<GeminiContent>,
    #[serde(skip_serializing_if = "Option::is_none")]
    system_instruction: Option<GeminiContent>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<Vec<Value>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_config: Option<GeminiToolConfig>,
    #[serde(skip_serializing_if = "Option::is_none")]
    generation_config: Option<GeminiGenerationConfig>,
    #[serde(skip_serializing_if = "Option::is_none")]
    cached_content: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
struct GeminiContent {
    #[serde(default)]
    role: String,
    #[serde(default)]
    parts: Vec<GeminiPart>,
}

#[derive(Debug, Serialize, Deserialize, Clone, Default)]
#[serde(rename_all = "camelCase")]
struct GeminiPart {
    #[serde(skip_serializing_if = "Option::is_none")]
    text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    inline_data: Option<GeminiBlob>,
    #[serde(skip_serializing_if = "Option::is_none")]
    file_data: Option<GeminiFileData>,
    #[serde(skip_serializing_if = "Option::is_none")]
    thought: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    thought_signature: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    function_call: Option<GeminiFunctionCall>,
    #[serde(skip_serializing_if = "Option::is_none")]
    function_response: Option<GeminiFunctionResponse>,
    #[serde(skip_serializing_if = "Option::is_none")]
    executable_code: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    code_execution_result: Option<Value>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
struct GeminiBlob {
    mime_type: String,
    data: String,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
struct GeminiFileData {
    #[serde(skip_serializing_if = "Option::is_none")]
    mime_type: Option<String>,
    file_uri: String,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
struct GeminiFunctionCall {
    #[serde(skip_serializing_if = "Option::is_none")]
    id: Option<String>,
    name: String,
    #[serde(default)]
    args: Value,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
struct GeminiFunctionResponse {
    #[serde(skip_serializing_if = "Option::is_none")]
    id: Option<String>,
    name: String,
    response: Value,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct GeminiFunctionDeclaration {
    name: String,
    description: String,
    parameters: Value,
}

#[derive(Debug, Serialize, Default)]
#[serde(rename_all = "camelCase")]
struct GeminiToolConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    function_calling_config: Option<GeminiFunctionCallingConfig>,
    #[serde(skip_serializing_if = "Option::is_none")]
    include_server_side_tool_invocations: Option<bool>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct GeminiFunctionCallingConfig {
    mode: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    allowed_function_names: Option<Vec<String>>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct GeminiGenerationConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    stop_sequences: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_output_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    top_p: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    top_k: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    thinking_config: Option<GeminiThinkingConfig>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct GeminiThinkingConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    include_thoughts: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    thinking_budget: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    thinking_level: Option<&'static str>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GeminiGenerateContentResponse {
    response_id: Option<String>,
    model_version: Option<String>,
    #[serde(default)]
    candidates: Vec<GeminiCandidate>,
    usage_metadata: Option<GeminiUsageMetadata>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GeminiCandidate {
    content: Option<GeminiContent>,
    finish_reason: Option<String>,
    grounding_metadata: Option<Value>,
    url_context_metadata: Option<Value>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GeminiUsageMetadata {
    #[serde(default)]
    prompt_token_count: u64,
    #[serde(default)]
    candidates_token_count: u64,
    total_token_count: Option<u64>,
    thoughts_token_count: Option<u64>,
    cached_content_token_count: Option<u64>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GeminiListModelsResponse {
    #[serde(default)]
    models: Vec<GeminiListModel>,
    #[serde(default)]
    publisher_models: Vec<GeminiListModel>,
    next_page_token: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GeminiListModel {
    name: String,
    base_model_id: Option<String>,
    #[serde(default)]
    supported_generation_methods: Vec<String>,
    #[serde(default)]
    supported_actions: Vec<String>,
}

// --- Encoding & Decoding helpers ---

/// Gemini 2.x models take a token budget (`thinkingBudget`), Gemini 3 and later a level
/// (`thinkingLevel`). Pro models cannot turn thinking off: "off" becomes their lowest setting.
fn gemini_thinking(model: &str, t: &crate::ThinkingConfig) -> Option<GeminiThinkingConfig> {
    let m = model.trim_start_matches("models/").to_ascii_lowercase();
    let budget_based = m.starts_with("gemini-2") || m.starts_with("gemini-1");
    let pro = m.contains("-pro");
    if !t.enabled {
        return match (budget_based, pro) {
            (true, false) => Some(GeminiThinkingConfig {
                include_thoughts: Some(false),
                thinking_budget: Some(0),
                thinking_level: None,
            }),
            (true, true) => Some(GeminiThinkingConfig {
                include_thoughts: Some(false),
                thinking_budget: Some(128),
                thinking_level: None,
            }),
            (false, _) => Some(GeminiThinkingConfig {
                include_thoughts: Some(false),
                thinking_budget: None,
                thinking_level: Some(if pro { "LOW" } else { "MINIMAL" }),
            }),
        };
    }
    let (budget, level) = match (t.budget_tokens, t.effort) {
        (Some(b), _) => (Some(b), None),
        (None, Some(e)) if budget_based => (Some(gemini_budget(e)), None),
        (None, Some(e)) => (None, Some(map_gemini_thinking_level(e))),
        (None, None) => (None, None),
    };
    Some(GeminiThinkingConfig {
        include_thoughts: Some(true),
        thinking_budget: budget,
        thinking_level: level,
    })
}

fn gemini_budget(effort: ReasoningEffort) -> u32 {
    match effort {
        ReasoningEffort::Low => 1_024,
        ReasoningEffort::Medium => 8_192,
        ReasoningEffort::High => 24_576,
    }
}

fn map_gemini_thinking_level(effort: ReasoningEffort) -> &'static str {
    match effort {
        ReasoningEffort::Low => "LOW",
        ReasoningEffort::Medium => "MEDIUM",
        ReasoningEffort::High => "HIGH",
    }
}

fn encode_gemini_function_declaration(tool: &ToolDefinition) -> GeminiFunctionDeclaration {
    GeminiFunctionDeclaration {
        name: tool.name.clone(),
        description: tool.description.clone(),
        parameters: tool.parameters.clone(),
    }
}

fn encode_gemini_tool_choice(choice: &ToolChoice) -> GeminiToolConfig {
    let function_calling_config = match choice {
        ToolChoice::Auto => GeminiFunctionCallingConfig {
            mode: "AUTO",
            allowed_function_names: None,
        },
        ToolChoice::None => GeminiFunctionCallingConfig {
            mode: "NONE",
            allowed_function_names: None,
        },
        ToolChoice::Required => GeminiFunctionCallingConfig {
            mode: "ANY",
            allowed_function_names: None,
        },
        ToolChoice::Tool { name } => GeminiFunctionCallingConfig {
            mode: "ANY",
            allowed_function_names: Some(vec![name.clone()]),
        },
    };
    GeminiToolConfig {
        function_calling_config: Some(function_calling_config),
        include_server_side_tool_invocations: None,
    }
}

fn build_generation_config(
    request: &ChatRequest,
    thinking_config: Option<GeminiThinkingConfig>,
) -> Option<GeminiGenerationConfig> {
    let stop_sequences = if request.stop_sequences.is_empty() {
        None
    } else {
        Some(request.stop_sequences.clone())
    };

    if stop_sequences.is_none()
        && request.max_tokens.is_none()
        && request.temperature.is_none()
        && request.top_p.is_none()
        && request.top_k.is_none()
        && thinking_config.is_none()
    {
        return None;
    }

    Some(GeminiGenerationConfig {
        stop_sequences,
        max_output_tokens: request.max_tokens,
        temperature: request.temperature,
        top_p: request.top_p,
        top_k: request.top_k,
        thinking_config,
    })
}

/// Documented placeholder for history without a signature (older turns, other models).
const SKIP_SIGNATURE: &str = "skip_thought_signature_validator";

/// Gemini 3+ rejects a model turn whose first function call has no `thoughtSignature`. Turns
/// recorded before signatures were kept per call, or produced by another model, get the
/// documented placeholder so the conversation can continue.
fn ensure_call_signature(model: &str, parts: &mut [GeminiPart]) {
    let m = model.trim_start_matches("models/").to_ascii_lowercase();
    if m.starts_with("gemini-1") || m.starts_with("gemini-2") {
        return;
    }
    if let Some(first) = parts.iter_mut().find(|p| p.function_call.is_some())
        && first.thought_signature.as_deref().is_none_or(str::is_empty)
    {
        first.thought_signature = Some(SKIP_SIGNATURE.to_string());
    }
}

fn encode_gemini_message(
    msg: &Message,
    call_names: &BTreeMap<String, String>,
) -> (String, Vec<GeminiPart>) {
    let role = match msg.role {
        Role::Assistant => "model".to_string(),
        Role::User | Role::Tool | Role::System => "user".to_string(),
    };

    let mut parts = Vec::new();
    for part in &msg.content {
        match part {
            ContentPart::Text { text, .. } => {
                parts.push(GeminiPart {
                    text: Some(text.clone()),
                    ..Default::default()
                });
            }
            ContentPart::Image { source } => match source {
                ImageSource::Base64 { media_type, data } => {
                    parts.push(GeminiPart {
                        inline_data: Some(GeminiBlob {
                            mime_type: media_type.clone(),
                            data: data.clone(),
                        }),
                        ..Default::default()
                    });
                }
                ImageSource::Url { url } => {
                    parts.push(GeminiPart {
                        file_data: Some(GeminiFileData {
                            mime_type: None,
                            file_uri: url.clone(),
                        }),
                        ..Default::default()
                    });
                }
            },
            ContentPart::Document { source, .. } => match source {
                DocumentSource::Base64 { media_type, data } => {
                    parts.push(GeminiPart {
                        inline_data: Some(GeminiBlob {
                            mime_type: media_type.clone(),
                            data: data.clone(),
                        }),
                        ..Default::default()
                    });
                }
                DocumentSource::Url { url, media_type } => {
                    parts.push(GeminiPart {
                        file_data: Some(GeminiFileData {
                            mime_type: media_type.clone(),
                            file_uri: url.clone(),
                        }),
                        ..Default::default()
                    });
                }
                DocumentSource::FileId {
                    file_id,
                    media_type,
                } => {
                    parts.push(GeminiPart {
                        file_data: Some(GeminiFileData {
                            mime_type: media_type.clone(),
                            file_uri: file_id.clone(),
                        }),
                        ..Default::default()
                    });
                }
            },
            ContentPart::Thinking {
                thinking,
                signature,
            } => {
                parts.push(GeminiPart {
                    text: if thinking.is_empty() {
                        None
                    } else {
                        Some(thinking.clone())
                    },
                    thought: Some(true),
                    thought_signature: signature.clone(),
                    ..Default::default()
                });
            }
            ContentPart::ToolCall(call) => {
                let args = match &call.arguments {
                    Value::Object(_) => call.arguments.clone(),
                    other => json!({ "input": other }),
                };
                parts.push(GeminiPart {
                    thought_signature: call.signature.clone(),
                    function_call: Some(GeminiFunctionCall {
                        id: if call.id.is_empty() {
                            None
                        } else {
                            Some(call.id.clone())
                        },
                        name: call.name.clone(),
                        args,
                    }),
                    ..Default::default()
                });
            }
            ContentPart::ToolResult(result) => {
                let name = result
                    .name
                    .clone()
                    .or_else(|| call_names.get(&result.tool_call_id).cloned())
                    .unwrap_or_else(|| result.tool_call_id.clone());

                let response_obj = if result.is_error {
                    json!({ "error": result.content })
                } else {
                    match serde_json::from_str::<Value>(&result.content) {
                        Ok(Value::Object(map)) => Value::Object(map),
                        _ => json!({ "output": result.content }),
                    }
                };

                parts.push(GeminiPart {
                    function_response: Some(GeminiFunctionResponse {
                        id: if result.tool_call_id.is_empty() {
                            None
                        } else {
                            Some(result.tool_call_id.clone())
                        },
                        name,
                        response: response_obj,
                    }),
                    ..Default::default()
                });
            }
            ContentPart::ServerToolUse { .. } | ContentPart::ServerToolResult { .. } => {}
        }
    }

    (role, parts)
}

fn map_gemini_finish_reason(reason: &str, has_tool_calls: bool) -> StopReason {
    if has_tool_calls && reason == "STOP" {
        return StopReason::ToolUse;
    }
    match reason {
        "STOP" => StopReason::EndTurn,
        "MAX_TOKENS" => StopReason::MaxTokens,
        "SAFETY" | "RECITATION" | "BLOCKLIST" | "PROHIBITED_CONTENT" => StopReason::ContentFilter,
        other => StopReason::Other(other.to_string()),
    }
}

fn map_gemini_usage(u: GeminiUsageMetadata) -> Usage {
    Usage {
        input_tokens: u.prompt_token_count,
        output_tokens: u.candidates_token_count,
        total_tokens: u.total_token_count,
        reasoning_tokens: u.thoughts_token_count,
        cache_read_tokens: u.cached_content_token_count,
        cache_creation_tokens: None,
    }
}

fn parse_gemini_grounding_citations(meta: &Value) -> Vec<Citation> {
    let mut out = Vec::new();
    let Some(chunks) = meta.get("groundingChunks").and_then(Value::as_array) else {
        return out;
    };
    for chunk in chunks {
        if let Some(web) = chunk.get("web")
            && let Some(uri) = web.get("uri").and_then(Value::as_str)
            && !uri.trim().is_empty()
        {
            let title = web
                .get("title")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            if !out.iter().any(|c: &Citation| c.url == uri) {
                out.push(Citation {
                    domain: extract_domain(uri),
                    url: uri.to_string(),
                    title,
                    snippet: None,
                    raw_provider_payload: Some(chunk.clone()),
                });
            }
        }
    }
    out
}

fn parse_gemini_url_context_blocks(meta: &Value) -> Vec<ContentPart> {
    let mut out = Vec::new();
    let Some(entries) = meta.get("urlMetadata").and_then(Value::as_array) else {
        return out;
    };
    for (i, entry) in entries.iter().enumerate() {
        let url = entry
            .get("retrievedUrl")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let status = entry
            .get("urlRetrievalStatus")
            .and_then(Value::as_str)
            .unwrap_or("SUCCESS");
        let is_error = status.contains("ERROR") || status.contains("UNSAFE");
        let id = format!("url_ctx_{i}");
        out.push(ContentPart::ServerToolUse {
            id: id.clone(),
            name: "url_context".to_string(),
            action: Some("fetch".to_string()),
            input: json!({ "url": url }),
            raw_provider_payload: None,
        });
        out.push(ContentPart::ServerToolResult {
            tool_use_id: id,
            name: "url_context".to_string(),
            summary: format!("{url} ({status})"),
            is_error,
            raw_provider_payload: Some(entry.clone()),
        });
    }
    out
}

fn parse_gemini_response(resp: GeminiGenerateContentResponse) -> Result<ChatResponse, Error> {
    let Some(candidate) = resp.candidates.into_iter().next() else {
        return Err(Error::api("Gemini response contained no candidates"));
    };

    let mut content = String::new();
    let mut thinking: Option<String> = None;
    let mut thinking_signature: Option<String> = None;
    let mut tool_calls = Vec::new();
    let mut server_tool_blocks = Vec::new();
    let mut citations = Vec::new();

    if let Some(gm) = &candidate.grounding_metadata {
        if let Some(queries) = gm.get("webSearchQueries").and_then(Value::as_array)
            && !queries.is_empty()
        {
            server_tool_blocks.push(ContentPart::ServerToolUse {
                id: "google_search_0".to_string(),
                name: "google_search".to_string(),
                action: Some("search".to_string()),
                input: json!({ "queries": queries }),
                raw_provider_payload: Some(gm.clone()),
            });
        }
        citations = parse_gemini_grounding_citations(gm);
        if !citations.is_empty() {
            server_tool_blocks.push(ContentPart::ServerToolResult {
                tool_use_id: "google_search_0".to_string(),
                name: "google_search".to_string(),
                summary: format!("{} sources", citations.len()),
                is_error: false,
                raw_provider_payload: None,
            });
        }
    }
    if let Some(uc) = &candidate.url_context_metadata {
        server_tool_blocks.extend(parse_gemini_url_context_blocks(uc));
    }

    if let Some(cand_content) = candidate.content {
        for (pidx, part) in cand_content.parts.into_iter().enumerate() {
            // A signature on a function call belongs to that call; others to the thinking.
            // Gemini signatures are whole values per part: never concatenate them.
            let sig = part.thought_signature.filter(|s| !s.is_empty());
            if part.function_call.is_none()
                && let Some(sig) = &sig
            {
                thinking_signature = Some(sig.clone());
            }
            if let Some(text) = part.text {
                if part.thought.unwrap_or(false) {
                    if !text.is_empty() {
                        thinking.get_or_insert_with(String::new).push_str(&text);
                    }
                } else {
                    content.push_str(&text);
                }
            }
            if let Some(code) = part.executable_code {
                server_tool_blocks.push(ContentPart::ServerToolUse {
                    id: format!("code_exec_{pidx}"),
                    name: "code_execution".to_string(),
                    action: Some("execute".to_string()),
                    input: code.clone(),
                    raw_provider_payload: Some(code),
                });
            }
            if let Some(res) = part.code_execution_result {
                let outcome = res.get("outcome").and_then(Value::as_str).unwrap_or("OUTCOME_OK");
                let summary = res
                    .get("output")
                    .and_then(Value::as_str)
                    .unwrap_or(outcome)
                    .chars()
                    .take(160)
                    .collect();
                server_tool_blocks.push(ContentPart::ServerToolResult {
                    tool_use_id: format!("code_exec_{}", pidx.saturating_sub(1)),
                    name: "code_execution".to_string(),
                    summary,
                    is_error: outcome != "OUTCOME_OK",
                    raw_provider_payload: Some(res),
                });
            }
            if let Some(fc) = part.function_call {
                let idx = tool_calls.len();
                let id = fc.id.unwrap_or_else(|| format!("call_{idx}"));
                tool_calls.push(ToolCall {
                    id,
                    name: fc.name,
                    arguments: fc.args,
                    signature: sig.clone(),
                });
            }
        }
    }

    let stop_reason = candidate
        .finish_reason
        .as_deref()
        .map(|r| map_gemini_finish_reason(r, !tool_calls.is_empty()));

    Ok(ChatResponse {
        id: resp.response_id,
        model: resp.model_version,
        content,
        thinking,
        thinking_signature,
        tool_calls,
        server_tool_blocks,
        citations,
        stop_reason,
        usage: resp.usage_metadata.map(map_gemini_usage),
    })
}

struct GeminiStreamState<S> {
    sse: S,
    queue: VecDeque<Result<StreamEvent, Error>>,
    started: bool,
    next_tool_index: usize,
    finish_reason: Option<String>,
    usage: Option<Usage>,
    done: bool,
}

fn gemini_sse_to_events<S>(
    sse: S,
) -> impl Stream<Item = Result<StreamEvent, Error>> + Send + 'static
where
    S: Stream<Item = Result<crate::sse::SseEvent, Error>> + Send + Unpin + 'static,
{
    let state = GeminiStreamState {
        sse,
        queue: VecDeque::new(),
        started: false,
        next_tool_index: 0,
        finish_reason: None,
        usage: None,
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
                        finish_gemini_stream(&mut st);
                        continue;
                    }

                    let raw: Value = match serde_json::from_str(data) {
                        Ok(v) => v,
                        Err(e) => {
                            st.done = true;
                            return Some((Err(Error::Serialization(e)), st));
                        }
                    };

                    if raw.get("error").is_some() {
                        let msg = extract_error_message_from_value(&raw)
                            .unwrap_or_else(|| "Gemini stream error".to_string());
                        st.done = true;
                        return Some((Err(Error::api(msg)), st));
                    }

                    let chunk: GeminiGenerateContentResponse = match serde_json::from_value(raw) {
                        Ok(c) => c,
                        Err(e) => {
                            st.done = true;
                            return Some((Err(Error::Serialization(e)), st));
                        }
                    };

                    if !st.started && (chunk.response_id.is_some() || chunk.model_version.is_some())
                    {
                        st.started = true;
                        st.queue.push_back(Ok(StreamEvent::MessageStart {
                            id: chunk.response_id,
                            model: chunk.model_version,
                        }));
                    }

                    if let Some(candidate) = chunk.candidates.into_iter().next() {
                        if let Some(cand_content) = candidate.content {
                            for part in cand_content.parts {
                                let sig = part.thought_signature.filter(|s| !s.is_empty());
                                if part.function_call.is_none()
                                    && let Some(sig) = &sig
                                {
                                    st.queue.push_back(Ok(StreamEvent::ThinkingSignatureDelta(
                                        sig.clone(),
                                    )));
                                }
                                if let Some(text) = part.text
                                    && !text.is_empty()
                                {
                                    if part.thought.unwrap_or(false) {
                                        st.queue.push_back(Ok(StreamEvent::ThinkingDelta(text)));
                                    } else {
                                        st.queue.push_back(Ok(StreamEvent::TextDelta(text)));
                                    }
                                }
                                if let Some(code) = part.executable_code {
                                    st.queue.push_back(Ok(StreamEvent::ServerToolUse {
                                        id: "code_exec".to_string(),
                                        name: "code_execution".to_string(),
                                        action: Some("execute".to_string()),
                                        input: code.clone(),
                                        raw_provider_payload: Some(code),
                                    }));
                                }
                                if let Some(res) = part.code_execution_result {
                                    let outcome = res
                                        .get("outcome")
                                        .and_then(Value::as_str)
                                        .unwrap_or("OUTCOME_OK");
                                    let summary = res
                                        .get("output")
                                        .and_then(Value::as_str)
                                        .unwrap_or(outcome)
                                        .chars()
                                        .take(160)
                                        .collect();
                                    st.queue.push_back(Ok(StreamEvent::ServerToolResult {
                                        tool_use_id: "code_exec".to_string(),
                                        name: "code_execution".to_string(),
                                        summary,
                                        is_error: outcome != "OUTCOME_OK",
                                        raw_provider_payload: Some(res),
                                    }));
                                }
                                if let Some(fc) = part.function_call {
                                    let idx = st.next_tool_index;
                                    st.next_tool_index += 1;
                                    let id = fc.id.unwrap_or_else(|| format!("call_{idx}"));
                                    let args_str = serde_json::to_string(&fc.args)
                                        .unwrap_or_else(|_| "{}".to_string());
                                    st.queue.push_back(Ok(StreamEvent::ToolCallStart {
                                        index: idx,
                                        id,
                                        name: fc.name,
                                    }));
                                    st.queue.push_back(Ok(StreamEvent::ToolCallDelta {
                                        index: idx,
                                        arguments_delta: args_str,
                                    }));
                                    if let Some(signature) = sig {
                                        st.queue.push_back(Ok(StreamEvent::ToolCallSignature {
                                            index: idx,
                                            signature,
                                        }));
                                    }
                                    st.queue
                                        .push_back(Ok(StreamEvent::ToolCallEnd { index: idx }));
                                }
                            }
                        }

                        if let Some(gm) = &candidate.grounding_metadata {
                            if let Some(queries) =
                                gm.get("webSearchQueries").and_then(Value::as_array)
                                && !queries.is_empty()
                            {
                                st.queue.push_back(Ok(StreamEvent::ServerToolUse {
                                    id: "google_search_0".to_string(),
                                    name: "google_search".to_string(),
                                    action: Some("search".to_string()),
                                    input: json!({ "queries": queries }),
                                    raw_provider_payload: Some(gm.clone()),
                                }));
                            }
                            let cites = parse_gemini_grounding_citations(gm);
                            if !cites.is_empty() {
                                st.queue.push_back(Ok(StreamEvent::ServerToolResult {
                                    tool_use_id: "google_search_0".to_string(),
                                    name: "google_search".to_string(),
                                    summary: format!("{} sources", cites.len()),
                                    is_error: false,
                                    raw_provider_payload: None,
                                }));
                                for c in cites {
                                    st.queue.push_back(Ok(StreamEvent::Citation(c)));
                                }
                            }
                        }
                        if let Some(uc) = &candidate.url_context_metadata {
                            for block in parse_gemini_url_context_blocks(uc) {
                                match block {
                                    ContentPart::ServerToolUse {
                                        id,
                                        name,
                                        action,
                                        input,
                                        raw_provider_payload,
                                    } => {
                                        st.queue.push_back(Ok(StreamEvent::ServerToolUse {
                                            id,
                                            name,
                                            action,
                                            input,
                                            raw_provider_payload,
                                        }));
                                    }
                                    ContentPart::ServerToolResult {
                                        tool_use_id,
                                        name,
                                        summary,
                                        is_error,
                                        raw_provider_payload,
                                    } => {
                                        st.queue.push_back(Ok(StreamEvent::ServerToolResult {
                                            tool_use_id,
                                            name,
                                            summary,
                                            is_error,
                                            raw_provider_payload,
                                        }));
                                    }
                                    _ => {}
                                }
                            }
                        }

                        if let Some(reason) = candidate.finish_reason {
                            st.finish_reason = Some(reason);
                        }
                    }

                    if let Some(usage) = chunk.usage_metadata {
                        st.usage = Some(map_gemini_usage(usage));
                    }
                }
                Some(Err(err)) => {
                    st.done = true;
                    return Some((Err(err), st));
                }
                None => {
                    finish_gemini_stream(&mut st);
                }
            }
        }
    })
}

fn finish_gemini_stream<S>(st: &mut GeminiStreamState<S>) {
    st.done = true;
    if let Some(usage) = st.usage.take() {
        st.queue.push_back(Ok(StreamEvent::Usage(usage)));
    }
    let has_tools = st.next_tool_index > 0;
    let stop_reason = st
        .finish_reason
        .take()
        .as_deref()
        .map(|r| map_gemini_finish_reason(r, has_tools));
    st.queue.push_back(Ok(StreamEvent::Done { stop_reason }));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::collect_stream;
    use mockito::Server;

    #[tokio::test]
    async fn test_gemini_chat_thinking_and_function_call() {
        let mut server = Server::new_async().await;
        let mock = server
            .mock("POST", "/models/gemini-2.5-flash:generateContent")
            .match_header("x-goog-api-key", "gem-key")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                r#"{
                    "responseId": "gem_1",
                    "modelVersion": "gemini-2.5-flash",
                    "candidates": [{
                        "content": {
                            "role": "model",
                            "parts": [
                                {"text": "Thinking about weather", "thought": true, "thoughtSignature": "tsig_1"},
                                {"text": "Checking now."},
                                {"functionCall": {"name": "get_weather", "args": {"city": "Seoul"}}}
                            ]
                        },
                        "finishReason": "STOP"
                    }],
                    "usageMetadata": {
                        "promptTokenCount": 30,
                        "candidatesTokenCount": 12,
                        "totalTokenCount": 42,
                        "thoughtsTokenCount": 5,
                        "cachedContentTokenCount": 20
                    }
                }"#,
            )
            .create_async()
            .await;

        let client = GeminiClient::new("gem-key", "gemini-2.5-flash").with_base_url(server.url());
        let req = ChatRequest::new(vec![Message::user("Weather in Seoul?")])
            .with_system("You are helpful.")
            .with_tools(vec![ToolDefinition::new(
                "get_weather",
                "Get weather",
                json!({"type": "object", "properties": {"city": {"type": "string"}}}),
            )])
            .with_thinking(crate::ThinkingConfig::with_effort(ReasoningEffort::Low));

        let resp = client.chat(req).await.unwrap();
        mock.assert_async().await;

        assert_eq!(resp.id.as_deref(), Some("gem_1"));
        assert_eq!(resp.thinking.as_deref(), Some("Thinking about weather"));
        assert_eq!(resp.thinking_signature.as_deref(), Some("tsig_1"));
        assert_eq!(resp.content, "Checking now.");
        assert_eq!(resp.tool_calls.len(), 1);
        assert_eq!(resp.tool_calls[0].id, "call_0");
        assert_eq!(resp.tool_calls[0].name, "get_weather");
        assert_eq!(resp.tool_calls[0].arguments, json!({"city": "Seoul"}));
        assert_eq!(resp.stop_reason, Some(StopReason::ToolUse));

        let usage = resp.usage.unwrap();
        assert_eq!(usage.input_tokens, 30);
        assert_eq!(usage.output_tokens, 12);
        assert_eq!(usage.reasoning_tokens, Some(5));
        assert_eq!(usage.cache_read_tokens, Some(20));
    }

    #[tokio::test]
    async fn test_gemini_stream_and_vertex_ai_models() {
        let mut server = Server::new_async().await;
        let sse_body = concat!(
            "data: {\"responseId\":\"gem_s1\",\"modelVersion\":\"gemini-2.5-pro\",\"candidates\":[{\"content\":{\"role\":\"model\",\"parts\":[{\"text\":\"Step 1\",\"thought\":true}]}}]}\n\n",
            "data: {\"candidates\":[{\"content\":{\"role\":\"model\",\"parts\":[{\"text\":\"Hello \"}]}}]}\n\n",
            "data: {\"candidates\":[{\"content\":{\"role\":\"model\",\"parts\":[{\"text\":\"Gemini!\"}]},\"finishReason\":\"STOP\"}],\"usageMetadata\":{\"promptTokenCount\":10,\"candidatesTokenCount\":8,\"totalTokenCount\":18,\"cachedContentTokenCount\":6}}\n\n"
        );

        let stream_mock = server
            .mock(
                "POST",
                "/projects/p1/locations/us-central1/publishers/google/models/gemini-2.5-pro:streamGenerateContent?alt=sse",
            )
            .match_header("authorization", "Bearer v-tok")
            .with_status(200)
            .with_header("content-type", "text/event-stream")
            .with_body(sse_body)
            .create_async()
            .await;

        let models_mock = server
            .mock(
                "GET",
                "/projects/p1/locations/us-central1/publishers/google/models?pageSize=1000",
            )
            .match_header("authorization", "Bearer v-tok")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                r#"{"publisherModels":[{"name":"publishers/google/models/gemini-2.5-pro","baseModelId":"gemini-2.5-pro","supportedActions":["generateContent"]},{"name":"publishers/google/models/text-embedding-005","supportedActions":["embedContent"]}]}"#,
            )
            .create_async()
            .await;

        let client = GeminiClient::new_vertex_ai("p1", "us-central1", "gemini-2.5-pro", "v-tok")
            .with_base_url(server.url());

        let stream = client
            .chat_stream(ChatRequest::new(vec![Message::user("Hi")]))
            .await
            .unwrap();
        let resp = collect_stream(stream).await.unwrap();
        stream_mock.assert_async().await;

        assert_eq!(resp.id.as_deref(), Some("gem_s1"));
        assert_eq!(resp.thinking.as_deref(), Some("Step 1"));
        assert_eq!(resp.content, "Hello Gemini!");
        assert_eq!(resp.stop_reason, Some(StopReason::EndTurn));
        assert_eq!(resp.usage.unwrap().cache_read_tokens, Some(6));

        let models = client.list_models().await.unwrap();
        models_mock.assert_async().await;
        assert_eq!(models, vec!["gemini-2.5-pro"]);
    }

    #[tokio::test]
    async fn test_gemini_stream_error_and_empty_model_config() {
        let mut server = Server::new_async().await;
        let body = "data: {\"error\":{\"code\":429,\"message\":\"Resource exhausted\",\"status\":\"RESOURCE_EXHAUSTED\"}}\n\n";
        let mock = server
            .mock(
                "POST",
                "/models/gemini-2.5-flash:streamGenerateContent?alt=sse",
            )
            .with_status(200)
            .with_header("content-type", "text/event-stream")
            .with_body(body)
            .create_async()
            .await;

        let client = GeminiClient::new("gem-key", "gemini-2.5-flash").with_base_url(server.url());
        let stream = client
            .chat_stream(ChatRequest::new(vec![Message::user("hi")]))
            .await
            .unwrap();
        let err = collect_stream(stream).await.unwrap_err();
        mock.assert_async().await;
        assert!(
            err.to_string()
                .contains("RESOURCE_EXHAUSTED: Resource exhausted")
        );

        let empty_model = GeminiClient::new("gem-key", "");
        let cfg_err = empty_model
            .chat(ChatRequest::new(vec![Message::user("hi")]))
            .await
            .unwrap_err();
        assert!(matches!(cfg_err, Error::InvalidConfig(_)));
    }
}

#[cfg(test)]
mod signature_tests {
    //! Gemini 3 thought signatures on function calls: recorded per call, sent back on the same
    //! part, and a placeholder for history that has none.
    use super::*;
    use crate::{ContentPart, Message, Role};

    fn response(parts: Value) -> GeminiGenerateContentResponse {
        serde_json::from_value(json!({ "candidates": [{ "content": { "role": "model", "parts": parts }, "finishReason": "STOP" }] }))
            .unwrap()
    }

    #[test]
    fn signatures_on_function_calls_stay_with_their_call() {
        let r = parse_gemini_response(response(json!([
            { "text": "Let me check", "thought": true, "thoughtSignature": "t-sig" },
            { "functionCall": { "name": "render_widget", "args": {} }, "thoughtSignature": "fc-sig" },
            { "functionCall": { "name": "read_file", "args": { "path": "a" } } },
        ])))
        .unwrap();
        assert_eq!(r.tool_calls[0].signature.as_deref(), Some("fc-sig"));
        assert_eq!(r.tool_calls[1].signature, None);
        // Not concatenated with the call's signature.
        assert_eq!(r.thinking_signature.as_deref(), Some("t-sig"));
    }

    fn history(sig: Option<&str>) -> ChatRequest {
        let call =
            ToolCall::new("c1", "render_widget", json!({})).with_signature(sig.map(str::to_string));
        let call2 = ToolCall::new("c2", "read_file", json!({}));
        ChatRequest::new(vec![
            Message::user("hi"),
            Message {
                role: Role::Assistant,
                content: vec![
                    ContentPart::text("ok"),
                    ContentPart::ToolCall(call),
                    ContentPart::ToolCall(call2),
                ],
            },
        ])
    }

    fn call_sigs(model: &str, req: &ChatRequest) -> Vec<Option<String>> {
        let (_, body) = GeminiClient::new("k", model)
            .build_request_body(req)
            .unwrap();
        let v = serde_json::to_value(body).unwrap();
        v["contents"][1]["parts"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|p| p.get("functionCall").is_some())
            .map(|p| {
                p.get("thoughtSignature")
                    .and_then(Value::as_str)
                    .map(str::to_string)
            })
            .collect()
    }

    #[test]
    fn the_signature_goes_back_on_its_function_call() {
        assert_eq!(
            call_sigs("gemini-3-pro-preview", &history(Some("fc-sig"))),
            vec![Some("fc-sig".into()), None]
        );
    }

    #[test]
    fn history_without_signatures_gets_the_placeholder_on_gemini_3_only() {
        assert_eq!(
            call_sigs("gemini-3-flash", &history(None)),
            vec![Some(SKIP_SIGNATURE.into()), None]
        );
        assert_eq!(
            call_sigs("gemma-4-31b", &history(None)),
            vec![Some(SKIP_SIGNATURE.into()), None]
        );
        assert_eq!(
            call_sigs("gemini-2.5-flash", &history(None)),
            vec![None, None]
        );
    }
}
