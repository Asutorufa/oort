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
    /// `OORT_MCP_CONFIG`: `{"mcpServers": {...}}` file imported into the MCP server table at
    /// startup (servers whose name already exists are left alone).
    pub mcp_config: Option<PathBuf>,
    /// `OORT_WORKSPACE` (default `.`): root for the built-in file tools.
    pub workspace: PathBuf,
    /// `OORT_WEB_DIST`: serve the built frontend from this directory.
    pub web_dist: Option<PathBuf>,
    /// `OORT_PUBLIC_URL` (default `http://{OORT_ADDR}`): base URL browsers use to reach the
    /// server; the OAuth redirect URI is `{public_url}/api/mcp/oauth/callback`.
    pub public_url: String,
    /// `OORT_SKILLS_DIRS` (`:`-separated; default `~/.oort/skills` and `{workspace}/skills`).
    pub skill_dirs: Vec<PathBuf>,
}

impl ServerConfig {
    pub fn from_env() -> Result<Self, String> {
        let var = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty());
        let addr: SocketAddr = var("OORT_ADDR")
            .unwrap_or_else(|| "127.0.0.1:3000".into())
            .parse()
            .map_err(|e| format!("OORT_ADDR: {e}"))?;
        let workspace: PathBuf = var("OORT_WORKSPACE")
            .map(PathBuf::from)
            .unwrap_or_else(|| ".".into());
        let skill_dirs = match var("OORT_SKILLS_DIRS") {
            Some(v) => std::env::split_paths(&v).collect(),
            None => var("HOME")
                .map(|h| PathBuf::from(h).join(".oort/skills"))
                .into_iter()
                .chain([workspace.join("skills")])
                .collect(),
        };
        Ok(Self {
            public_url: var("OORT_PUBLIC_URL").unwrap_or_else(|| format!("http://{addr}")),
            skill_dirs,
            addr,
            db_url: var("OORT_DB").unwrap_or_else(|| "sqlite://oort.db".into()),
            mcp_config: var("OORT_MCP_CONFIG").map(PathBuf::from),
            workspace,
            web_dist: var("OORT_WEB_DIST").map(PathBuf::from),
        })
    }
}

/// An MCP server attached to a session, with the tool-name prefix it had when the session was
/// created (renaming the server later must not rename the session's frozen tools).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpBinding {
    pub id: String,
    pub prefix: String,
}

/// A skill attached to a session: its id and the name it was indexed under.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillBinding {
    pub id: String,
    pub name: String,
}

fn yes() -> bool {
    true
}

/// Immutable per-session configuration (stored in `sessions.config_json`).
///
/// Sessions created before the workbench have no `provider_id`; their `provider` is one of the
/// legacy kinds (`anthropic`, `openai` = Chat Completions, `openai_responses`, `gemini`, `mock`)
/// and their keys come from the environment.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionConfig {
    /// Provider kind at creation time.
    pub provider: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_id: Option<String>,
    pub model: String,
    pub max_context_tokens: u64,
    #[serde(default)]
    pub max_tokens: Option<u32>,
    #[serde(default = "yes")]
    pub builtin_tools: bool,
    #[serde(default)]
    pub mcp_servers: Vec<McpBinding>,
    /// `render_widget` is available (sessions created before widgets have none).
    #[serde(default)]
    pub widgets: bool,
    /// Skills whose index is frozen into the system prompt (`load_skill` serves their bodies).
    #[serde(default)]
    pub skills: Vec<SkillBinding>,
    /// Legacy approval mode (`mutating` / `never`); superseded by `sessions.approval_policy`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approval: Option<String>,
}

impl SessionConfig {
    pub fn is_legacy(&self) -> bool {
        self.provider_id.is_none()
    }

    /// Anthropic reports `input_tokens` excluding cache reads/writes; the others include them.
    pub fn accounting(&self) -> UsageAccounting {
        if self.provider == "anthropic" {
            UsageAccounting::Exclusive
        } else {
            UsageAccounting::Inclusive
        }
    }

    pub fn policy(&self) -> CompactionPolicy {
        let mut p = CompactionPolicy::new(self.max_context_tokens);
        p.accounting = self.accounting();
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_config_still_parses() {
        let raw = r#"{"approval":"never","max_context_tokens":200000,"max_tokens":null,"model":"m","provider":"gemini"}"#;
        let cfg: SessionConfig = serde_json::from_str(raw).unwrap();
        assert!(cfg.is_legacy());
        assert!(cfg.builtin_tools);
        assert!(cfg.mcp_servers.is_empty());
        assert_eq!(cfg.approval.as_deref(), Some("never"));
    }
}
