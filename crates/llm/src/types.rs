use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::Error;

/// Conversation participant role.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    System,
    User,
    Assistant,
    Tool,
}

/// Image payload for multimodal prompts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ImageSource {
    /// Remote HTTP(S) URL or data URL.
    Url { url: String },
    /// Base64-encoded raw image bytes with its MIME media type (e.g. `"image/png"`).
    Base64 { media_type: String, data: String },
}

/// Document / PDF payload for multimodal prompts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum DocumentSource {
    /// Base64-encoded raw document bytes with its MIME media type (e.g. `"application/pdf"`).
    Base64 { media_type: String, data: String },
    /// Remote HTTP(S) URL or GCS/Files URI.
    Url {
        url: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        media_type: Option<String>,
    },
    /// Provider-hosted file identifier (e.g. OpenAI `file-...`, Anthropic `file_...`, Gemini `files/...`).
    FileId {
        file_id: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        media_type: Option<String>,
    },
}

/// A single function/tool invocation requested by the model.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: Value,
    /// Opaque reasoning signature the provider attached to this call (Gemini 3 puts it on the
    /// first function call of a step and requires it back on that same part).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signature: Option<String>,
}

impl ToolCall {
    pub fn new(id: impl Into<String>, name: impl Into<String>, arguments: Value) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            arguments,
            signature: None,
        }
    }

    pub fn with_signature(mut self, signature: Option<String>) -> Self {
        self.signature = signature.filter(|s| !s.is_empty());
        self
    }

    /// Serializes `arguments` to a JSON string (used by APIs like OpenAI that pass arguments as strings).
    pub fn arguments_json_string(&self) -> String {
        match &self.arguments {
            Value::String(s) => s.clone(),
            other => serde_json::to_string(other).unwrap_or_else(|_| "{}".to_string()),
        }
    }
}

/// Result of executing a model-requested tool call.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolResult {
    pub tool_call_id: String,
    /// Tool/function name. Required by Gemini; optional for OpenAI and Anthropic.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub content: String,
    #[serde(default)]
    pub is_error: bool,
}

impl ToolResult {
    pub fn ok(
        tool_call_id: impl Into<String>,
        name: Option<impl Into<String>>,
        content: impl Into<String>,
    ) -> Self {
        Self {
            tool_call_id: tool_call_id.into(),
            name: name.map(Into::into),
            content: content.into(),
            is_error: false,
        }
    }

    pub fn error(
        tool_call_id: impl Into<String>,
        name: Option<impl Into<String>>,
        content: impl Into<String>,
    ) -> Self {
        Self {
            tool_call_id: tool_call_id.into(),
            name: name.map(Into::into),
            content: content.into(),
            is_error: true,
        }
    }
}

/// Vendor identity, orthogonal to wire protocol (`OpenAIClient`, `OpenAIResponsesClient`,
/// `AnthropicClient`, `GeminiClient`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Vendor {
    Openai,
    Anthropic,
    Gemini,
    Deepseek,
    Openrouter,
    Qwen,
    Ollama,
    #[default]
    Custom,
}

impl Vendor {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Openai => "openai",
            Self::Anthropic => "anthropic",
            Self::Gemini => "gemini",
            Self::Deepseek => "deepseek",
            Self::Openrouter => "openrouter",
            Self::Qwen => "qwen",
            Self::Ollama => "ollama",
            Self::Custom => "custom",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "openai" => Some(Self::Openai),
            "anthropic" => Some(Self::Anthropic),
            "gemini" => Some(Self::Gemini),
            "deepseek" => Some(Self::Deepseek),
            "openrouter" => Some(Self::Openrouter),
            "qwen" => Some(Self::Qwen),
            "ollama" => Some(Self::Ollama),
            "custom" => Some(Self::Custom),
            _ => None,
        }
    }
}

/// Configuration for provider-hosted web search.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WebSearchToolConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_uses: Option<u32>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allowed_domains: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub blocked_domains: Vec<String>,
}

/// Configuration for OpenAI Responses `file_search` vector store retrieval.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileSearchToolConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub vector_store_ids: Vec<String>,
}

/// Provider-hosted server-side tool capabilities enabled for a request or session.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServerToolConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub web_search: Option<WebSearchToolConfig>,
    #[serde(default)]
    pub url_context: bool,
    #[serde(default)]
    pub code_execution: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file_search: Option<FileSearchToolConfig>,
}

impl ServerToolConfig {
    pub fn web_search_enabled(&self) -> bool {
        self.web_search.as_ref().is_some_and(|w| w.enabled)
    }

    pub fn file_search_enabled(&self) -> bool {
        self.file_search
            .as_ref()
            .is_some_and(|f| f.enabled && !f.vector_store_ids.is_empty())
    }

    pub fn is_empty(&self) -> bool {
        !self.web_search_enabled()
            && !self.url_context
            && !self.code_execution
            && !self.file_search_enabled()
    }
}

/// Unified citation source attached to assistant text from provider-hosted search/grounding.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Citation {
    pub url: String,
    #[serde(default)]
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snippet: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub domain: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub raw_provider_payload: Option<Value>,
}

impl Citation {
    pub fn new(url: impl Into<String>, title: impl Into<String>) -> Self {
        let url = url.into();
        let domain = extract_domain(&url);
        Self {
            url,
            title: title.into(),
            snippet: None,
            domain,
            raw_provider_payload: None,
        }
    }
}

pub fn extract_domain(url: &str) -> Option<String> {
    url::Url::parse(url)
        .ok()
        .and_then(|u| u.host_str().map(|h| h.trim_start_matches("www.").to_string()))
}

/// A typed content block inside a [`Message`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ContentPart {
    Text {
        text: String,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        citations: Vec<Citation>,
    },
    Image {
        source: ImageSource,
    },
    Document {
        source: DocumentSource,
        #[serde(skip_serializing_if = "Option::is_none")]
        filename: Option<String>,
    },
    Thinking {
        thinking: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        signature: Option<String>,
    },
    ToolCall(ToolCall),
    ToolResult(ToolResult),
    /// Provider-hosted server tool invocation (never executed locally).
    ServerToolUse {
        id: String,
        name: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        action: Option<String>,
        #[serde(default)]
        input: Value,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        raw_provider_payload: Option<Value>,
    },
    /// Provider-hosted server tool result (preserves opaque encrypted payloads for replay).
    ServerToolResult {
        tool_use_id: String,
        name: String,
        #[serde(default)]
        summary: String,
        #[serde(default)]
        is_error: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        raw_provider_payload: Option<Value>,
    },
}

impl ContentPart {
    pub fn text(text: impl Into<String>) -> Self {
        Self::Text {
            text: text.into(),
            citations: Vec::new(),
        }
    }

    pub fn text_with_citations(text: impl Into<String>, citations: Vec<Citation>) -> Self {
        Self::Text {
            text: text.into(),
            citations,
        }
    }

    pub fn image_url(url: impl Into<String>) -> Self {
        Self::Image {
            source: ImageSource::Url { url: url.into() },
        }
    }

    pub fn image_base64(media_type: impl Into<String>, data: impl Into<String>) -> Self {
        Self::Image {
            source: ImageSource::Base64 {
                media_type: media_type.into(),
                data: data.into(),
            },
        }
    }

    pub fn pdf_base64(data: impl Into<String>, filename: Option<impl Into<String>>) -> Self {
        Self::Document {
            source: DocumentSource::Base64 {
                media_type: "application/pdf".to_string(),
                data: data.into(),
            },
            filename: filename.map(Into::into),
        }
    }

    pub fn pdf_url(url: impl Into<String>) -> Self {
        Self::Document {
            source: DocumentSource::Url {
                url: url.into(),
                media_type: Some("application/pdf".to_string()),
            },
            filename: None,
        }
    }

    pub fn document_base64(
        media_type: impl Into<String>,
        data: impl Into<String>,
        filename: Option<impl Into<String>>,
    ) -> Self {
        Self::Document {
            source: DocumentSource::Base64 {
                media_type: media_type.into(),
                data: data.into(),
            },
            filename: filename.map(Into::into),
        }
    }

    pub fn document_url(url: impl Into<String>, media_type: Option<impl Into<String>>) -> Self {
        Self::Document {
            source: DocumentSource::Url {
                url: url.into(),
                media_type: media_type.map(Into::into),
            },
            filename: None,
        }
    }

    pub fn document_file_id(
        file_id: impl Into<String>,
        media_type: Option<impl Into<String>>,
    ) -> Self {
        Self::Document {
            source: DocumentSource::FileId {
                file_id: file_id.into(),
                media_type: media_type.map(Into::into),
            },
            filename: None,
        }
    }

    pub fn thinking(thinking: impl Into<String>, signature: Option<String>) -> Self {
        Self::Thinking {
            thinking: thinking.into(),
            signature,
        }
    }
}

/// A single turn in a conversation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Message {
    pub role: Role,
    pub content: Vec<ContentPart>,
}

impl Message {
    pub fn new(role: Role, content: Vec<ContentPart>) -> Self {
        Self { role, content }
    }

    pub fn system(text: impl Into<String>) -> Self {
        Self::new(Role::System, vec![ContentPart::text(text)])
    }

    pub fn user(text: impl Into<String>) -> Self {
        Self::new(Role::User, vec![ContentPart::text(text)])
    }

    pub fn assistant(text: impl Into<String>) -> Self {
        Self::new(Role::Assistant, vec![ContentPart::text(text)])
    }

    pub fn user_parts(parts: Vec<ContentPart>) -> Self {
        Self::new(Role::User, parts)
    }

    pub fn assistant_parts(parts: Vec<ContentPart>) -> Self {
        Self::new(Role::Assistant, parts)
    }

    pub fn tool_result(
        tool_call_id: impl Into<String>,
        name: Option<impl Into<String>>,
        content: impl Into<String>,
        is_error: bool,
    ) -> Self {
        Self::new(
            Role::Tool,
            vec![ContentPart::ToolResult(ToolResult {
                tool_call_id: tool_call_id.into(),
                name: name.map(Into::into),
                content: content.into(),
                is_error,
            })],
        )
    }

    /// Concatenates all [`ContentPart::Text`] parts in this message.
    pub fn text(&self) -> String {
        let mut out = String::new();
        for part in &self.content {
            if let ContentPart::Text { text, .. } = part {
                out.push_str(text);
            }
        }
        out
    }
}

/// JSON Schema definition of a callable tool/function.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    /// JSON Schema object describing the tool's parameters.
    pub parameters: Value,
}

impl ToolDefinition {
    pub fn new(
        name: impl Into<String>,
        description: impl Into<String>,
        parameters: Value,
    ) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
            parameters,
        }
    }
}

/// Controls how the model chooses among provided tools.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ToolChoice {
    Auto,
    None,
    Required,
    Tool { name: String },
}

/// Reasoning effort hint for thinking-capable models.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ReasoningEffort {
    Low,
    Medium,
    High,
}

/// Configuration for extended thinking / reasoning models.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ThinkingConfig {
    pub enabled: bool,
    /// Token budget for thinking (used by Anthropic and Gemini `thinkingBudget`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub budget_tokens: Option<u32>,
    /// Qualitative reasoning effort (used by OpenAI and Gemini `thinkingLevel`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub effort: Option<ReasoningEffort>,
}

impl ThinkingConfig {
    pub fn enabled() -> Self {
        Self {
            enabled: true,
            budget_tokens: None,
            effort: None,
        }
    }

    pub fn with_budget(budget_tokens: u32) -> Self {
        Self {
            enabled: true,
            budget_tokens: Some(budget_tokens),
            effort: None,
        }
    }

    pub fn with_effort(effort: ReasoningEffort) -> Self {
        Self {
            enabled: true,
            budget_tokens: None,
            effort: Some(effort),
        }
    }

    pub fn disabled() -> Self {
        Self {
            enabled: false,
            budget_tokens: None,
            effort: None,
        }
    }
}

/// Cache lifetime hint for prompt/prefix caching.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CacheTtl {
    /// Short-lived cache (Anthropic default 5m ephemeral, OpenAI in-memory).
    FiveMinutes,
    /// Extended 1-hour cache (Anthropic `ttl: "1h"`).
    OneHour,
    /// Extended 24-hour cache (OpenAI `prompt_cache_retention: "24h"`).
    TwentyFourHours,
}

/// Prompt / prefix caching configuration applied across providers.
///
/// - **Anthropic**: attaches `cache_control` breakpoints to the last tool, the system prompt,
///   and the last block of the latest message (never exceeding Anthropic's 4-breakpoint limit).
/// - **OpenAI / OpenAI Responses**: forwards `prompt_cache_key` (for sticky cache routing)
///   and `prompt_cache_retention` when `ttl` is `TwentyFourHours`.
/// - **Gemini**: forwards explicit `cached_content` resource name when provided; otherwise relies
///   on deterministic `systemInstruction` + `tools` + `contents` ordering for implicit caching.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct PromptCacheConfig {
    /// Enables provider cache hints (e.g. Anthropic `cache_control` breakpoints).
    pub enabled: bool,
    /// Optional cache routing key (OpenAI `prompt_cache_key`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    /// Optional cache TTL hint.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ttl: Option<CacheTtl>,
    /// Optional explicit cached content resource name (Gemini `cachedContents/...`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cached_content: Option<String>,
}

impl PromptCacheConfig {
    pub fn enabled() -> Self {
        Self {
            enabled: true,
            ..Default::default()
        }
    }

    pub fn with_key(mut self, key: impl Into<String>) -> Self {
        self.enabled = true;
        self.key = Some(key.into());
        self
    }

    pub fn with_ttl(mut self, ttl: CacheTtl) -> Self {
        self.enabled = true;
        self.ttl = Some(ttl);
        self
    }

    pub fn with_cached_content(mut self, resource_name: impl Into<String>) -> Self {
        self.enabled = true;
        self.cached_content = Some(resource_name.into());
        self
    }
}

/// Provider-agnostic prompt cache boundary.
///
/// Adapters that support explicit breakpoints (Anthropic `cache_control`) translate these into
/// provider markers; adapters with implicit/automatic prefix caching (OpenAI, Gemini) ignore them.
/// When [`ChatRequest::cache_breakpoints`] is empty, adapters fall back to their default placement.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum CacheBreakpoint {
    /// End of the static prefix: tool definitions and the system prompt.
    ToolsAndSystem,
    /// End of `messages[index]` (index into [`ChatRequest::messages`]).
    AfterMessage { index: usize },
}

/// Provider-agnostic chat completion request.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ChatRequest {
    /// Optional model override; if `None`, the client's configured default model is used.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Optional top-level system instruction (merged with any `Role::System` messages).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system: Option<String>,
    pub messages: Vec<Message>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<ToolDefinition>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_choice: Option<ToolChoice>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_k: Option<u32>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub stop_sequences: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thinking: Option<ThinkingConfig>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prompt_cache: Option<PromptCacheConfig>,
    /// Explicit, provider-agnostic cache boundaries (only honored when `prompt_cache` is enabled).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub cache_breakpoints: Vec<CacheBreakpoint>,
    /// Provider-hosted server tools (web search, URL context, code execution, file search).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub server_tools: Option<ServerToolConfig>,
    /// Ask the provider to also return a conversation title (DeepSeek `title.enabled`; adapters
    /// send nothing for other vendors). A top-level parameter outside the cached prefix.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub generate_title: bool,
    /// Optional top-level JSON fields merged into the request body (escape hatch for custom vendors).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extra_body: Option<Value>,
}

impl ChatRequest {
    pub fn new(messages: Vec<Message>) -> Self {
        Self {
            messages,
            ..Default::default()
        }
    }

    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        self.model = Some(model.into());
        self
    }

    pub fn with_system(mut self, system: impl Into<String>) -> Self {
        self.system = Some(system.into());
        self
    }

    pub fn with_tools(mut self, tools: Vec<ToolDefinition>) -> Self {
        self.tools = tools;
        self
    }

    pub fn with_tool_choice(mut self, tool_choice: ToolChoice) -> Self {
        self.tool_choice = Some(tool_choice);
        self
    }

    pub fn with_max_tokens(mut self, max_tokens: u32) -> Self {
        self.max_tokens = Some(max_tokens);
        self
    }

    pub fn with_temperature(mut self, temperature: f32) -> Self {
        self.temperature = Some(temperature);
        self
    }

    pub fn with_top_p(mut self, top_p: f32) -> Self {
        self.top_p = Some(top_p);
        self
    }

    pub fn with_top_k(mut self, top_k: u32) -> Self {
        self.top_k = Some(top_k);
        self
    }

    pub fn with_stop_sequences(mut self, stop_sequences: Vec<String>) -> Self {
        self.stop_sequences = stop_sequences;
        self
    }

    pub fn with_thinking(mut self, thinking: ThinkingConfig) -> Self {
        self.thinking = Some(thinking);
        self
    }

    pub fn with_prompt_cache(mut self, prompt_cache: PromptCacheConfig) -> Self {
        self.prompt_cache = Some(prompt_cache);
        self
    }

    pub fn with_cache_breakpoints(mut self, breakpoints: Vec<CacheBreakpoint>) -> Self {
        self.cache_breakpoints = breakpoints;
        self
    }

    pub fn with_server_tools(mut self, server_tools: ServerToolConfig) -> Self {
        self.server_tools = Some(server_tools);
        self
    }

    pub fn with_extra_body(mut self, extra_body: Value) -> Self {
        self.extra_body = Some(extra_body);
        self
    }

    /// Resolves the effective model name or returns `Error::InvalidConfig` if empty.
    pub(crate) fn resolve_model<'a>(&'a self, default_model: &'a str) -> Result<&'a str, Error> {
        let model = self
            .model
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| default_model.trim());
        if model.is_empty() {
            return Err(Error::InvalidConfig("model name must not be empty".to_string()));
        }
        Ok(model)
    }

    /// Combines `self.system` and any `Role::System` messages into a single system prompt,
    /// returning `(combined_system, non_system_messages)`.
    pub(crate) fn split_system_messages(&self) -> (Option<String>, Vec<&Message>) {
        let mut system_parts = Vec::new();
        if let Some(sys) = &self.system
            && !sys.is_empty()
        {
            system_parts.push(sys.clone());
        }

        let mut non_system = Vec::with_capacity(self.messages.len());
        for message in &self.messages {
            if message.role == Role::System {
                let text = message.text();
                if !text.is_empty() {
                    system_parts.push(text);
                }
            } else {
                non_system.push(message);
            }
        }

        let system = if system_parts.is_empty() {
            None
        } else {
            Some(system_parts.join("\n\n"))
        };
        (system, non_system)
    }
}

/// Token usage statistics reported by the model.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Usage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_read_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_creation_tokens: Option<u64>,
}

/// Normalized reason why generation stopped.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StopReason {
    EndTurn,
    MaxTokens,
    ToolUse,
    StopSequence,
    ContentFilter,
    Other(String),
}

/// Complete non-streaming (or collected streaming) chat response.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ChatResponse {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    pub content: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thinking: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thinking_signature: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<ToolCall>,
    /// Chronological provider-hosted server tool uses and results (never executed locally).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub server_tool_blocks: Vec<ContentPart>,
    /// Unified citations extracted from provider-hosted search/grounding.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub citations: Vec<Citation>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stop_reason: Option<StopReason>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage: Option<Usage>,
    /// Conversation title the provider generated (see [`ChatRequest::generate_title`]). Never
    /// part of the history.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
}

impl ChatResponse {
    /// Converts this response into an assistant [`Message`] suitable for appending to
    /// conversation history in a multi-turn agent loop.
    pub fn into_assistant_message(self) -> Message {
        let mut parts = Vec::new();
        if let Some(thinking) = self.thinking {
            if !thinking.is_empty() || self.thinking_signature.is_some() {
                parts.push(ContentPart::Thinking {
                    thinking,
                    signature: self.thinking_signature,
                });
            }
        } else if let Some(signature) = self.thinking_signature {
            parts.push(ContentPart::Thinking {
                thinking: String::new(),
                signature: Some(signature),
            });
        }
        parts.extend(self.server_tool_blocks);
        if !self.content.is_empty() || !self.citations.is_empty() {
            parts.push(ContentPart::Text {
                text: self.content,
                citations: self.citations,
            });
        }
        for call in self.tool_calls {
            parts.push(ContentPart::ToolCall(call));
        }
        Message::assistant_parts(parts)
    }
}

/// Incremental event emitted by [`crate::LlmClient::chat_stream`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum StreamEvent {
    /// Initial response metadata (id, model).
    MessageStart {
        #[serde(skip_serializing_if = "Option::is_none")]
        id: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        model: Option<String>,
    },
    /// Incremental assistant text output.
    TextDelta(String),
    /// Incremental reasoning / chain-of-thought text.
    ThinkingDelta(String),
    /// Incremental cryptographic/opaque thinking signature (Anthropic / Gemini).
    ThinkingSignatureDelta(String),
    /// Start of a streamed tool call at `index`.
    ToolCallStart {
        index: usize,
        id: String,
        name: String,
    },
    /// Incremental JSON arguments fragment for the tool call at `index`.
    ToolCallDelta {
        index: usize,
        arguments_delta: String,
    },
    /// Marks completion of the tool call at `index`.
    ToolCallEnd { index: usize },
    /// Reasoning signature attached to the tool call at `index` (Gemini).
    ToolCallSignature { index: usize, signature: String },
    /// Provider-hosted server tool invocation observed in stream.
    ServerToolUse {
        id: String,
        name: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        action: Option<String>,
        #[serde(default)]
        input: Value,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        raw_provider_payload: Option<Value>,
    },
    /// Provider-hosted server tool result observed in stream.
    ServerToolResult {
        tool_use_id: String,
        name: String,
        #[serde(default)]
        summary: String,
        #[serde(default)]
        is_error: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        raw_provider_payload: Option<Value>,
    },
    /// Citation source observed in stream.
    Citation(Citation),
    /// Token usage update.
    Usage(Usage),
    /// Provider-generated conversation title.
    Title(String),
    /// Stream completion marker with optional stop reason.
    Done {
        #[serde(skip_serializing_if = "Option::is_none")]
        stop_reason: Option<StopReason>,
    },
}

#[derive(Debug, Default)]
struct PendingToolCall {
    id: String,
    name: String,
    signature: Option<String>,
    arguments_buf: String,
}

/// Accumulates [`StreamEvent`] items into a complete [`ChatResponse`].
#[derive(Debug, Default)]
pub struct StreamCollector {
    id: Option<String>,
    model: Option<String>,
    content: String,
    thinking: Option<String>,
    thinking_signature: Option<String>,
    tool_calls: BTreeMap<usize, PendingToolCall>,
    server_tool_blocks: Vec<ContentPart>,
    citations: Vec<Citation>,
    stop_reason: Option<StopReason>,
    usage: Option<Usage>,
    title: Option<String>,
}

impl StreamCollector {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, event: &StreamEvent) {
        match event {
            StreamEvent::MessageStart { id, model } => {
                if id.is_some() {
                    self.id = id.clone();
                }
                if model.is_some() {
                    self.model = model.clone();
                }
            }
            StreamEvent::TextDelta(text) => {
                self.content.push_str(text);
            }
            StreamEvent::ThinkingDelta(delta) => {
                if !delta.is_empty() {
                    self.thinking.get_or_insert_with(String::new).push_str(delta);
                }
            }
            StreamEvent::ThinkingSignatureDelta(sig) => {
                if !sig.is_empty() {
                    self.thinking_signature
                        .get_or_insert_with(String::new)
                        .push_str(sig);
                }
            }
            StreamEvent::ToolCallStart { index, id, name } => {
                let entry = self.tool_calls.entry(*index).or_default();
                if !id.is_empty() {
                    entry.id = id.clone();
                }
                if !name.is_empty() {
                    entry.name = name.clone();
                }
            }
            StreamEvent::ToolCallDelta {
                index,
                arguments_delta,
            } => {
                let entry = self.tool_calls.entry(*index).or_default();
                entry.arguments_buf.push_str(arguments_delta);
            }
            StreamEvent::ToolCallEnd { .. } => {}
            StreamEvent::ToolCallSignature { index, signature } => {
                if !signature.is_empty() {
                    self.tool_calls.entry(*index).or_default().signature = Some(signature.clone());
                }
            }
            StreamEvent::ServerToolUse {
                id,
                name,
                action,
                input,
                raw_provider_payload,
            } => {
                self.server_tool_blocks.push(ContentPart::ServerToolUse {
                    id: id.clone(),
                    name: name.clone(),
                    action: action.clone(),
                    input: input.clone(),
                    raw_provider_payload: raw_provider_payload.clone(),
                });
            }
            StreamEvent::ServerToolResult {
                tool_use_id,
                name,
                summary,
                is_error,
                raw_provider_payload,
            } => {
                self.server_tool_blocks.push(ContentPart::ServerToolResult {
                    tool_use_id: tool_use_id.clone(),
                    name: name.clone(),
                    summary: summary.clone(),
                    is_error: *is_error,
                    raw_provider_payload: raw_provider_payload.clone(),
                });
            }
            StreamEvent::Citation(c) => {
                if !self.citations.iter().any(|existing| existing.url == c.url) {
                    self.citations.push(c.clone());
                }
            }
            StreamEvent::Usage(usage) => {
                self.usage = Some(usage.clone());
            }
            StreamEvent::Title(title) => {
                self.title = Some(title.clone());
            }
            StreamEvent::Done { stop_reason } => {
                if stop_reason.is_some() {
                    self.stop_reason = stop_reason.clone();
                }
            }
        }
    }

    /// Finalizes collection and parses accumulated tool call argument strings as JSON.
    pub fn finish(self) -> Result<ChatResponse, Error> {
        let mut tool_calls = Vec::with_capacity(self.tool_calls.len());
        for (_, pending) in self.tool_calls {
            let arguments = parse_tool_arguments(&pending.arguments_buf)?;
            tool_calls.push(ToolCall {
                id: pending.id,
                name: pending.name,
                arguments,
                signature: pending.signature,
            });
        }

        Ok(ChatResponse {
            id: self.id,
            model: self.model,
            content: self.content,
            thinking: self.thinking,
            thinking_signature: self.thinking_signature,
            tool_calls,
            server_tool_blocks: self.server_tool_blocks,
            citations: self.citations,
            stop_reason: self.stop_reason,
            usage: self.usage,
            title: self.title,
        })
    }
}

/// Adds DeepSeek's native title request to a request body. Other vendors reject the field.
pub(crate) fn apply_title_request(vendor: Vendor, request: &ChatRequest, body: &mut Value) {
    if vendor == Vendor::Deepseek
        && request.generate_title
        && let Some(obj) = body.as_object_mut()
    {
        obj.insert("title".to_string(), json!({ "enabled": true }));
    }
}

/// A non-empty title string from a response field of any JSON type.
pub(crate) fn title_of(value: Option<&Value>) -> Option<String> {
    value
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .map(ToString::to_string)
}

/// Parses a tool call JSON arguments string into a [`serde_json::Value`].
/// Empty or whitespace-only strings default to `{}`.
pub(crate) fn parse_tool_arguments(raw: &str) -> Result<Value, Error> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Ok(json!({}));
    }
    Ok(serde_json::from_str(trimmed)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_request_split_system_messages() {
        let req = ChatRequest::new(vec![
            Message::system("Rule 1"),
            Message::user("Hello"),
            Message::system("Rule 2"),
            Message::assistant("Hi"),
        ])
        .with_system("Base system");

        let (sys, msgs) = req.split_system_messages();
        assert_eq!(sys.as_deref(), Some("Base system\n\nRule 1\n\nRule 2"));
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[0].role, Role::User);
        assert_eq!(msgs[1].role, Role::Assistant);
    }

    #[test]
    fn test_stream_collector_assembles_full_response() {
        let mut collector = StreamCollector::new();
        collector.push(&StreamEvent::MessageStart {
            id: Some("msg_1".to_string()),
            model: Some("test-model".to_string()),
        });
        collector.push(&StreamEvent::ThinkingDelta("Step 1. ".to_string()));
        collector.push(&StreamEvent::ThinkingDelta("Step 2.".to_string()));
        collector.push(&StreamEvent::ThinkingSignatureDelta("sig_abc".to_string()));
        collector.push(&StreamEvent::TextDelta("Calling tool".to_string()));
        collector.push(&StreamEvent::ToolCallStart {
            index: 0,
            id: "call_1".to_string(),
            name: "get_weather".to_string(),
        });
        collector.push(&StreamEvent::ToolCallDelta {
            index: 0,
            arguments_delta: "{\"city\":".to_string(),
        });
        collector.push(&StreamEvent::ToolCallDelta {
            index: 0,
            arguments_delta: "\"Tokyo\"}".to_string(),
        });
        collector.push(&StreamEvent::ToolCallEnd { index: 0 });
        collector.push(&StreamEvent::Usage(Usage {
            input_tokens: 10,
            output_tokens: 20,
            total_tokens: Some(30),
            ..Default::default()
        }));
        collector.push(&StreamEvent::Done {
            stop_reason: Some(StopReason::ToolUse),
        });

        let resp = collector.finish().unwrap();
        assert_eq!(resp.id.as_deref(), Some("msg_1"));
        assert_eq!(resp.model.as_deref(), Some("test-model"));
        assert_eq!(resp.thinking.as_deref(), Some("Step 1. Step 2."));
        assert_eq!(resp.thinking_signature.as_deref(), Some("sig_abc"));
        assert_eq!(resp.content, "Calling tool");
        assert_eq!(resp.tool_calls.len(), 1);
        assert_eq!(resp.tool_calls[0].id, "call_1");
        assert_eq!(resp.tool_calls[0].name, "get_weather");
        assert_eq!(resp.tool_calls[0].arguments, json!({"city": "Tokyo"}));
        assert_eq!(resp.stop_reason, Some(StopReason::ToolUse));
        assert_eq!(resp.usage.as_ref().unwrap().total_tokens, Some(30));

        let assistant_msg = resp.into_assistant_message();
        assert_eq!(assistant_msg.role, Role::Assistant);
        assert_eq!(assistant_msg.content.len(), 3);
    }

    #[test]
    fn test_parse_tool_arguments_empty_defaults_to_empty_object() {
        assert_eq!(parse_tool_arguments("").unwrap(), json!({}));
        assert_eq!(parse_tool_arguments("   ").unwrap(), json!({}));
        assert!(parse_tool_arguments("{invalid").is_err());
    }
}
