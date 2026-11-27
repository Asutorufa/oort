//! Assembles agents: provider client, shared tool executor, context manager, hooks.

use std::sync::Arc;
use std::time::Duration;

use oort_agent::{
    Agent, AgentEvent, AgentState, ApprovalPolicy, CompositeToolExecutor, FrozenToolset,
    LlmCompactor, ModelClient, Namespace, PersistenceHook, RetryingModel, RuleBasedCompactor,
    SessionStore, ToolExecutor, TracingHook,
};
use oort_llm::{AnthropicClient, GeminiClient, OpenAIClient, OpenAIResponsesClient};
use oort_mcp::{ClientOptions, McpClient, McpConfig};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::api::ApiError;
use crate::config::{ApprovalMode, ProviderKind, ServerConfig, SessionConfig};
use crate::mock_model::MockModel;
use crate::tools::builtin_executor;

/// Holds process-wide resources shared by all sessions: the tool executor (built-in tools +
/// MCP servers, connected once) and its frozen catalog.
pub struct AgentFactory {
    executor: Arc<dyn ToolExecutor>,
    frozen: FrozenToolset,
    mcp_clients: Vec<Arc<McpClient>>,
    mock_word_delay: Duration,
}

impl AgentFactory {
    /// Built-in tools plus every MCP server in `OORT_MCP_CONFIG` (prefixed `{server}__`).
    pub async fn from_config(cfg: &ServerConfig) -> Result<Self, String> {
        let mut builder = CompositeToolExecutor::builder()
            .add(Namespace::None, Arc::new(builtin_executor(&cfg.workspace)));
        let mut clients = Vec::new();
        if let Some(path) = &cfg.mcp_config {
            let raw =
                std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
            let mcp = McpConfig::from_json(&raw).map_err(|e| format!("{}: {e}", path.display()))?;
            let (b, c) = mcp
                .connect_into(builder, &ClientOptions::default())
                .await
                .map_err(|e| e.to_string())?;
            builder = b;
            clients = c;
        }
        let executor: Arc<dyn ToolExecutor> = Arc::new(builder.build().map_err(|e| e.to_string())?);
        let mut f = Self::with_executor(executor)?;
        f.mcp_clients = clients;
        Ok(f)
    }

    pub fn with_executor(executor: Arc<dyn ToolExecutor>) -> Result<Self, String> {
        let frozen = FrozenToolset::freeze(executor.as_ref()).map_err(|e| e.to_string())?;
        Ok(Self {
            executor,
            frozen,
            mcp_clients: vec![],
            mock_word_delay: Duration::from_millis(30),
        })
    }

    pub fn with_mock_word_delay(mut self, d: Duration) -> Self {
        self.mock_word_delay = d;
        self
    }

    pub fn frozen(&self) -> &FrozenToolset {
        &self.frozen
    }

    pub async fn close(&self) {
        for c in &self.mcp_clients {
            c.close().await;
        }
    }

    /// Provider client from environment API keys (`*_API_KEY`, optional `*_BASE_URL`).
    pub fn model(&self, cfg: &SessionConfig) -> Result<Arc<dyn ModelClient>, ApiError> {
        let key = |name: &str| {
            std::env::var(name)
                .map_err(|_| ApiError::BadRequest(format!("{name} is not set on the server")))
        };
        let base = |name: &str| std::env::var(name).ok().filter(|v| !v.is_empty());
        let m = cfg.model.clone();
        let inner: Arc<dyn ModelClient> = match cfg.provider {
            ProviderKind::Mock => {
                return Ok(Arc::new(MockModel {
                    word_delay: self.mock_word_delay,
                }));
            }
            ProviderKind::Anthropic => {
                let mut c = AnthropicClient::new(key("ANTHROPIC_API_KEY")?, m);
                if let Some(b) = base("ANTHROPIC_BASE_URL") {
                    c = c.with_base_url(b);
                }
                Arc::new(c)
            }
            ProviderKind::Openai => {
                let mut c = OpenAIClient::new(key("OPENAI_API_KEY")?, m);
                if let Some(b) = base("OPENAI_BASE_URL") {
                    c = c.with_base_url(b);
                }
                Arc::new(c)
            }
            ProviderKind::OpenaiResponses => {
                let mut c = OpenAIResponsesClient::new(key("OPENAI_API_KEY")?, m);
                if let Some(b) = base("OPENAI_BASE_URL") {
                    c = c.with_base_url(b);
                }
                Arc::new(c)
            }
            ProviderKind::Gemini => {
                let mut c = GeminiClient::new(key("GEMINI_API_KEY")?, m);
                if let Some(b) = base("GEMINI_BASE_URL") {
                    c = c.with_base_url(b);
                }
                Arc::new(c)
            }
        };
        Ok(Arc::new(RetryingModel::new(
            inner,
            3,
            Duration::from_millis(500),
        )))
    }

    /// One agent per session (hooks carry the session id and journal).
    pub fn build_agent(
        &self,
        cfg: &SessionConfig,
        session_id: &str,
        state: &AgentState,
        store: Arc<dyn SessionStore>,
        events: mpsc::Sender<AgentEvent>,
        cancel: CancellationToken,
    ) -> Result<Agent, ApiError> {
        let model = self.model(cfg)?;
        let mut b = Agent::builder(model.clone(), Arc::clone(&self.executor))
            .context_manager(Arc::new(cfg.context_manager(session_id)))
            .hook(Arc::new(TracingHook))
            .events(events)
            .session_cancel(cancel);
        b = match cfg.provider {
            ProviderKind::Mock => b.compactor(Arc::new(RuleBasedCompactor {
                max_chars_per_message: 120,
            })),
            _ => b.compactor(Arc::new(LlmCompactor::new(model))),
        };
        if cfg.approval == ApprovalMode::Mutating {
            b = b.hook(Arc::new(ApprovalPolicy::default()));
        }
        // Persistence runs last so it records what the other hooks decided.
        b = b.hook(Arc::new(PersistenceHook::new(
            store,
            session_id.to_string(),
            state,
        )));
        b.build().map_err(|e| ApiError::Internal(e.to_string()))
    }
}
