//! Database-agnostic storage of workbench configuration: LLM providers, MCP servers and
//! presets (agent templates).
//!
//! The engine never reads this. Applications resolve a record into a `ModelClient` /
//! `ToolExecutor` when a session is created or resumed. Editing or deleting a record never
//! touches existing sessions: their system prompt and tool catalog are frozen in the session
//! store.
//!
//! Secret fields (`api_key`, header values, stdio env values, bearer tokens) are stored as given:
//! either a literal or an `env:NAME` reference (see [`crate::secret`]).

use std::collections::BTreeMap;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::store::StoreError;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderKind {
    /// OpenAI (Responses API).
    Openai,
    Anthropic,
    Gemini,
    /// Any server implementing OpenAI Chat Completions (`base_url` required).
    OpenaiCompatible,
    /// Offline echo model (for trying the workbench without keys).
    Mock,
}

impl ProviderKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Openai => "openai",
            Self::Anthropic => "anthropic",
            Self::Gemini => "gemini",
            Self::OpenaiCompatible => "openai_compatible",
            Self::Mock => "mock",
        }
    }

    pub fn parse(s: &str) -> Result<Self, StoreError> {
        serde_json::from_value(serde_json::Value::String(s.to_string()))
            .map_err(|_| StoreError::Corrupt(format!("unknown provider kind {s:?}")))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderRecord {
    pub id: String,
    pub name: String,
    pub kind: ProviderKind,
    pub base_url: Option<String>,
    pub api_key: Option<String>,
    pub default_model: String,
    pub extra_headers: BTreeMap<String, String>,
    pub is_enabled: bool,
    /// Unix milliseconds.
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum McpTransportKind {
    StreamableHttp,
    Stdio,
}

impl McpTransportKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::StreamableHttp => "streamable_http",
            Self::Stdio => "stdio",
        }
    }

    pub fn parse(s: &str) -> Result<Self, StoreError> {
        serde_json::from_value(serde_json::Value::String(s.to_string()))
            .map_err(|_| StoreError::Corrupt(format!("unknown MCP transport {s:?}")))
    }
}

/// Authentication of a Streamable HTTP server. `OAuth` is a reserved extension point: its
/// fields are stored verbatim until a flow is implemented.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum McpAuth {
    #[default]
    None,
    Bearer {
        token: String,
    },
    #[serde(rename = "oauth")]
    OAuth {
        #[serde(flatten)]
        config: BTreeMap<String, serde_json::Value>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpServerRecord {
    pub id: String,
    /// Also the tool-name prefix (`{name}__{tool}`) inside sessions.
    pub name: String,
    pub transport: McpTransportKind,
    pub url: Option<String>,
    pub command: Option<String>,
    pub args: Vec<String>,
    pub env: BTreeMap<String, String>,
    pub headers: BTreeMap<String, String>,
    pub auth: McpAuth,
    pub is_enabled: bool,
    pub created_at: i64,
    pub updated_at: i64,
}

/// Which tool calls need a human decision. Not part of the request prefix, so it may change
/// during a session.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalPolicyKind {
    /// Everything runs without asking.
    AutoAll,
    /// Read-only tools run; others ask.
    #[default]
    AutoReadonly,
    /// Every tool call asks.
    AlwaysAsk,
}

impl ApprovalPolicyKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::AutoAll => "auto_all",
            Self::AutoReadonly => "auto_readonly",
            Self::AlwaysAsk => "always_ask",
        }
    }

    pub fn parse(s: &str) -> Result<Self, StoreError> {
        serde_json::from_value(serde_json::Value::String(s.to_string()))
            .map_err(|_| StoreError::Corrupt(format!("unknown approval policy {s:?}")))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PresetRecord {
    pub id: String,
    pub name: String,
    pub description: String,
    pub provider_id: String,
    /// Empty: the provider's default model.
    pub model: String,
    pub system_prompt: String,
    pub mcp_server_ids: Vec<String>,
    /// Include the server's built-in local tools.
    pub builtin_tools: bool,
    pub approval_policy: ApprovalPolicyKind,
    /// Skills whose index is frozen into new sessions (DB ids or `file:<name>`).
    #[serde(default)]
    pub skill_ids: Vec<String>,
    /// Builtin tool groups for new sessions.
    #[serde(default)]
    pub builtin_groups: Vec<String>,
    /// Workspace root for the `workspace` group.
    #[serde(default)]
    pub workspace_root: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
}

/// Configuration persistence. `create_*` fails with `Conflict` on a duplicate id or name,
/// `update_*` with `NotFound` for an unknown id, and `delete_*` with `Conflict` while a preset
/// still references the record.
#[async_trait]
pub trait ConfigStore: Send + Sync {
    async fn list_providers(&self) -> Result<Vec<ProviderRecord>, StoreError>;
    async fn get_provider(&self, id: &str) -> Result<ProviderRecord, StoreError>;
    async fn create_provider(&self, rec: &ProviderRecord) -> Result<(), StoreError>;
    async fn update_provider(&self, rec: &ProviderRecord) -> Result<(), StoreError>;
    async fn delete_provider(&self, id: &str) -> Result<(), StoreError>;

    async fn list_mcp_servers(&self) -> Result<Vec<McpServerRecord>, StoreError>;
    async fn get_mcp_server(&self, id: &str) -> Result<McpServerRecord, StoreError>;
    async fn create_mcp_server(&self, rec: &McpServerRecord) -> Result<(), StoreError>;
    async fn update_mcp_server(&self, rec: &McpServerRecord) -> Result<(), StoreError>;
    async fn delete_mcp_server(&self, id: &str) -> Result<(), StoreError>;

    async fn list_presets(&self) -> Result<Vec<PresetRecord>, StoreError>;
    async fn get_preset(&self, id: &str) -> Result<PresetRecord, StoreError>;
    async fn create_preset(&self, rec: &PresetRecord) -> Result<(), StoreError>;
    async fn update_preset(&self, rec: &PresetRecord) -> Result<(), StoreError>;
    async fn delete_preset(&self, id: &str) -> Result<(), StoreError>;
}

// ----- MCP OAuth -----------------------------------------------------------------------------

/// How the OAuth client was obtained.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OAuthClientSource {
    /// Dynamic Client Registration (RFC 7591).
    Dcr,
    /// `client_id` / `client_secret` configured on the server record.
    Manual,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OAuthStatus {
    Connected,
    /// Refresh failed or no token yet: the user must authorize again.
    AuthRequired,
}

/// OAuth state of one MCP server. Kept apart from [`McpServerRecord`] so that token refreshes
/// do not change the server record (whose `updated_at` versions pooled connections).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OAuthTokenRecord {
    pub server_id: String,
    pub issuer: String,
    /// Canonical resource URI (RFC 8707) the tokens are bound to.
    pub resource: String,
    pub token_endpoint: String,
    pub registration_endpoint: Option<String>,
    pub client_id: String,
    pub client_secret: Option<String>,
    pub client_source: OAuthClientSource,
    pub access_token: Option<String>,
    pub refresh_token: Option<String>,
    /// Unix milliseconds.
    pub expires_at: Option<i64>,
    /// Space-separated, stored in full (never truncated).
    pub scopes: String,
    pub status: OAuthStatus,
    pub updated_at: i64,
}

#[async_trait]
pub trait OAuthTokenStore: Send + Sync {
    async fn get_oauth_token(
        &self,
        server_id: &str,
    ) -> Result<Option<OAuthTokenRecord>, StoreError>;
    async fn put_oauth_token(&self, rec: &OAuthTokenRecord) -> Result<(), StoreError>;
    async fn delete_oauth_token(&self, server_id: &str) -> Result<(), StoreError>;
}

// ----- skills --------------------------------------------------------------------------------

/// A skill stored in the database (file-based skills live in `SKILL.md` files).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillRecord {
    pub id: String,
    /// `[a-z0-9-]+`, unique; also the `/name` command.
    pub name: String,
    pub description: String,
    /// Markdown instructions, loaded on demand.
    pub body: String,
    pub allowed_tools: Vec<String>,
    pub tags: Vec<String>,
    #[serde(default)]
    pub invocation: SkillInvocation,
    pub created_at: i64,
    pub updated_at: i64,
}

/// How a skill can be invoked explicitly (`/name`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillInvocation {
    /// Offered as `/name` in the composer.
    pub user_invocable: bool,
    /// Shown after the command, e.g. `<domain>`.
    pub argument_hint: Option<String>,
    pub execution_mode: SkillExecutionMode,
    /// Subagent profile for `execution_mode = subagent`.
    pub subagent_profile: Option<String>,
}

impl Default for SkillInvocation {
    fn default() -> Self {
        Self {
            user_invocable: true,
            argument_hint: None,
            execution_mode: SkillExecutionMode::Inline,
            subagent_profile: None,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SkillExecutionMode {
    /// The body is appended to the user's message.
    #[default]
    Inline,
    /// An isolated subagent follows the skill; only its summary returns.
    Subagent,
}

#[async_trait]
pub trait SkillStore: Send + Sync {
    async fn list_skills(&self) -> Result<Vec<SkillRecord>, StoreError>;
    async fn get_skill(&self, id: &str) -> Result<SkillRecord, StoreError>;
    async fn create_skill(&self, rec: &SkillRecord) -> Result<(), StoreError>;
    async fn update_skill(&self, rec: &SkillRecord) -> Result<(), StoreError>;
    async fn delete_skill(&self, id: &str) -> Result<(), StoreError>;
}

// ----- memories ------------------------------------------------------------------------------

/// Cross-session memory (preferences, conventions, project facts). Searched on demand; never
/// injected into prompts automatically.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoryRecord {
    pub id: String,
    pub title: String,
    pub content: String,
    pub tags: Vec<String>,
    /// `global`, or a workspace / project identifier.
    pub scope: String,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoryQuery {
    /// Full-text query (empty: most recent first).
    pub text: String,
    /// Every tag must be present.
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub scope: Option<String>,
    #[serde(default)]
    pub limit: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct MemoryHit {
    #[serde(flatten)]
    pub memory: MemoryRecord,
    /// Relevance (higher is better; 0 without a text query).
    pub score: f64,
}

#[async_trait]
pub trait MemoryStore: Send + Sync {
    async fn search_memories(&self, q: &MemoryQuery) -> Result<Vec<MemoryHit>, StoreError>;
    async fn get_memory(&self, id: &str) -> Result<MemoryRecord, StoreError>;
    async fn create_memory(&self, rec: &MemoryRecord) -> Result<(), StoreError>;
    async fn update_memory(&self, rec: &MemoryRecord) -> Result<(), StoreError>;
    async fn delete_memory(&self, id: &str) -> Result<(), StoreError>;
}
