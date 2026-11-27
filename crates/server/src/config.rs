//! Server and per-session configuration.

use std::net::SocketAddr;
use std::path::PathBuf;

use oort_agent::{CompactionPolicy, DefaultContextManager, RequestParams, UsageAccounting};
use oort_llm::PromptCacheConfig;
use serde::{Deserialize, Serialize};

/// Process-level configuration, read from environment variables.
#[derive(Debug, Clone)]
pub struct ServerConfig {
    /// `OORT_ADDR` (default `127.0.0.1:3000`).
    pub addr: SocketAddr,
    /// `OORT_DB` (default `sqlite://oort.db`).
    pub db_url: String,
    /// `OORT_MCP_CONFIG`: path to a `{"mcpServers": {...}}` JSON file.
    pub mcp_config: Option<PathBuf>,
    /// `OORT_WORKSPACE` (default `.`): root for the built-in file tools.
    pub workspace: PathBuf,
    /// `OORT_WEB_DIST`: serve the built frontend from this directory.
    pub web_dist: Option<PathBuf>,
}

impl ServerConfig {
    pub fn from_env() -> Result<Self, String> {
        let var = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty());
        Ok(Self {
            addr: var("OORT_ADDR")
                .unwrap_or_else(|| "127.0.0.1:3000".into())
                .parse()
                .map_err(|e| format!("OORT_ADDR: {e}"))?,
            db_url: var("OORT_DB").unwrap_or_else(|| "sqlite://oort.db".into()),
            mcp_config: var("OORT_MCP_CONFIG").map(PathBuf::from),
            workspace: var("OORT_WORKSPACE")
                .map(PathBuf::from)
                .unwrap_or_else(|| ".".into()),
            web_dist: var("OORT_WEB_DIST").map(PathBuf::from),
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderKind {
    Anthropic,
    Openai,
    OpenaiResponses,
    Gemini,
    /// Offline echo model with simulated streaming and cache usage.
    Mock,
}

impl ProviderKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Anthropic => "anthropic",
            Self::Openai => "openai",
            Self::OpenaiResponses => "openai_responses",
            Self::Gemini => "gemini",
            Self::Mock => "mock",
        }
    }

    /// Anthropic reports `input_tokens` excluding cache reads/writes; the others include them.
    pub fn accounting(self) -> UsageAccounting {
        match self {
            Self::Anthropic => UsageAccounting::Exclusive,
            _ => UsageAccounting::Inclusive,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalMode {
    /// Tools without `readOnlyHint` / read-only flag need human approval.
    #[default]
    Mutating,
    /// Everything runs without approval.
    Never,
}

/// Immutable per-session configuration (stored in `sessions.config_json`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionConfig {
    pub provider: ProviderKind,
    pub model: String,
    pub max_context_tokens: u64,
    #[serde(default)]
    pub max_tokens: Option<u32>,
    #[serde(default)]
    pub approval: ApprovalMode,
}

impl SessionConfig {
    pub fn policy(&self) -> CompactionPolicy {
        let mut p = CompactionPolicy::new(self.max_context_tokens);
        p.accounting = self.provider.accounting();
        p
    }

    pub fn context_manager(&self, session_id: &str) -> DefaultContextManager {
        DefaultContextManager::new(RequestParams {
            model: Some(self.model.clone()),
            max_tokens: Some(self.max_tokens.unwrap_or(4096)),
            prompt_cache: Some(PromptCacheConfig::enabled().with_key(session_id)),
            ..Default::default()
        })
        .with_compaction(self.policy())
    }
}
