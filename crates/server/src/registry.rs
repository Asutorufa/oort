//! Turns stored configuration into runtime objects: provider records → `ModelClient`,
//! MCP records + built-in tools → `ToolExecutor`, and assembles per-session agents.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use oort_agent::secret::{self, SecretError};
use oort_agent::{
    Agent, AgentEvent, AgentState, CompositeToolExecutor, ConfigStore, LlmCompactor,
    McpServerRecord, ModelClient, Namespace, OAuthTokenStore, PersistenceHook, PinnedToolExecutor,
    ProviderKind, ProviderRecord, RetryingModel, RuleBasedCompactor, SessionStore, SkillStore,
    ToolExecutor, TracingHook,
};
use oort_llm::{
    AnthropicClient, ChatRequest, GeminiClient, LlmClient, Message, OpenAIClient,
    OpenAIResponsesClient, Provider,
};
use oort_mcp::{ClientOptions, McpConfig, McpError, ServerEntry};
use oort_skills::SkillRegistry;
use serde::Serialize;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::api::ApiError;
use crate::approval::{ApprovalHandle, DynamicApproval};
use crate::config::{McpBinding, SessionConfig};
use crate::mock_model::MockModel;
use crate::oauth::OAuthFlows;
use crate::pool::McpPool;
use crate::session_tools;
use crate::subagent::{SpawnSubagent, SubagentLimits, SubagentRuntime};
use oort_agent::{Lineage, MemoryStore};
use oort_tools::builtin::utilities::WebFetchOptions;
use oort_tools::{BuiltinContext, Group, PlanSink};

/// Group names → groups (unknown names are an error).
pub fn parse_groups(names: &[String]) -> Result<Vec<Group>, ApiError> {
    let mut out = Vec::new();
    for n in names {
        let g = Group::parse(n)
            .ok_or_else(|| ApiError::BadRequest(format!("unknown builtin tool group `{n}`")))?;
        if !out.contains(&g) {
            out.push(g);
        }
    }
    out.sort();
    Ok(out)
}

/// A provider client: a real HTTP client or the offline mock.
enum Built {
    Llm(Box<Provider>),
    Mock,
}

fn llm(p: impl Into<Provider>) -> Built {
    Built::Llm(Box::new(p.into()))
}

/// Result of `POST /api/providers/:id/test`.
#[derive(Debug, Clone, Serialize)]
pub struct ProviderTest {
    pub ok: bool,
    pub latency_ms: u64,
    pub model_checked: Option<String>,
    /// Whether `model_checked` is in the provider's model list (`None` if not listed).
    pub model_found: Option<bool>,
    pub models: Vec<String>,
    pub error: Option<String>,
}

pub struct Registry {
    pub config: Arc<dyn ConfigStore>,
    pub pool: McpPool,
    pub skills: Arc<SkillRegistry>,
    pub oauth: OAuthFlows,
    pub memories: Arc<dyn MemoryStore>,
    pub subagents: SubagentLimits,
    builtin: Arc<dyn ToolExecutor>,
    mock_word_delay: Duration,
    web: WebFetchOptions,
}

/// Everything the registry stores (one SQLite database in practice).
pub trait Stores: ConfigStore + OAuthTokenStore + SkillStore + MemoryStore + 'static {}
impl<T: ConfigStore + OAuthTokenStore + SkillStore + MemoryStore + 'static> Stores for T {}

/// Sends `update_plan` steps into the session's event stream.
struct EventPlanSink(mpsc::Sender<AgentEvent>);

impl PlanSink for EventPlanSink {
    fn publish(&self, steps: Vec<oort_agent::PlanStep>) {
        let _ = self.0.try_send(AgentEvent::PlanUpdated { steps });
    }
}

/// What a session's tools are made of (fixed at creation, in its config).
pub struct ToolSelection<'a> {
    pub builtin: bool,
    pub widgets: bool,
    /// Skill names (for `load_skill`).
    pub skills: Vec<String>,
    pub mcp: &'a [McpBinding],
    /// Builtin tool groups (see `oort_tools`).
    pub groups: Vec<Group>,
    pub workspace_root: Option<std::path::PathBuf>,
    /// The running session (absent while only freezing a catalog).
    pub runtime: Option<SubagentRuntime>,
    /// Subagent depth of the session the tools are for.
    pub depth: u32,
}

fn secret_err(e: SecretError) -> ApiError {
    ApiError::BadRequest(e.to_string())
}

fn mcp_err(name: &str, e: McpError) -> ApiError {
    match e {
        McpError::Config(m) => ApiError::BadRequest(format!("MCP server `{name}`: {m}")),
        other => ApiError::BadGateway(format!("MCP server `{name}`: {other}")),
    }
}

impl Registry {
    /// No skill directories and `http://127.0.0.1:3000` as public URL until configured.
    pub fn new<S: Stores>(store: Arc<S>, builtin: Arc<dyn ToolExecutor>) -> Self {
        let tokens: Arc<dyn OAuthTokenStore> = store.clone();
        Self {
            memories: store.clone(),
            subagents: SubagentLimits::default(),
            web: WebFetchOptions::default(),
            config: store.clone(),
            pool: McpPool::new(ClientOptions::default(), Some(tokens.clone())),
            skills: Arc::new(SkillRegistry::new(vec![], store)),
            oauth: OAuthFlows::new(tokens, "http://127.0.0.1:3000"),
            builtin,
            mock_word_delay: Duration::from_millis(30),
        }
    }

    /// `web_fetch` limits (tests allow private addresses to reach a local server).
    pub fn with_web_fetch(mut self, web: WebFetchOptions) -> Self {
        self.web = web;
        self
    }

    pub fn with_subagent_limits(mut self, limits: SubagentLimits) -> Self {
        self.subagents = limits;
        self
    }

    /// Context for the builtin tools of one session.
    pub fn builtin_context(
        &self,
        workspace_root: Option<std::path::PathBuf>,
        events: Option<mpsc::Sender<AgentEvent>>,
    ) -> BuiltinContext {
        BuiltinContext {
            config: Some(self.config.clone()),
            skills: Some(self.skills.store().clone()),
            memories: Some(self.memories.clone()),
            plan: events.map(|e| Arc::new(EventPlanSink(e)) as Arc<dyn PlanSink>),
            workspace_root,
            web: self.web.clone(),
            commands: Default::default(),
        }
    }

    /// Directories scanned for `<skill>/SKILL.md`.
    pub fn with_skill_dirs(mut self, dirs: Vec<std::path::PathBuf>) -> Self {
        let store = self.skills.store().clone();
        self.skills = Arc::new(SkillRegistry::new(dirs, store));
        self
    }

    /// Base URL browsers use to reach this server (OAuth redirect URI).
    pub fn with_public_url(mut self, url: &str) -> Self {
        self.oauth = OAuthFlows::new(self.oauth.store().clone(), url);
        self
    }

    pub fn with_mock_word_delay(mut self, d: Duration) -> Self {
        self.mock_word_delay = d;
        self
    }

    pub fn builtin(&self) -> &Arc<dyn ToolExecutor> {
        &self.builtin
    }

    // ----- providers -------------------------------------------------------------------

    /// Validates that a client can be built (keys resolve, headers are valid) without calling it.
    pub fn check_provider(rec: &ProviderRecord, model: &str) -> Result<(), ApiError> {
        Self::build(rec, model).map(|_| ())
    }

    fn build(rec: &ProviderRecord, model: &str) -> Result<Built, ApiError> {
        let key = rec
            .api_key
            .as_deref()
            .filter(|k| !k.is_empty())
            .map(secret::resolve)
            .transpose()
            .map_err(secret_err)?;
        let need_key = || {
            key.clone().ok_or_else(|| {
                ApiError::BadRequest(format!("provider `{}` has no API key", rec.name))
            })
        };
        let headers = secret::resolve_map(&rec.extra_headers).map_err(secret_err)?;
        let base = rec
            .base_url
            .as_deref()
            .map(str::trim)
            .filter(|b| !b.is_empty());
        let header_err = |e: oort_llm::Error| ApiError::BadRequest(format!("extra header: {e}"));
        macro_rules! configure {
            ($client:expr) => {{
                let mut c = $client;
                if let Some(b) = base {
                    c = c.with_base_url(b);
                }
                for (k, v) in &headers {
                    c = c.with_header(k, v).map_err(header_err)?;
                }
                c
            }};
        }
        Ok(match rec.kind {
            ProviderKind::Mock => Built::Mock,
            ProviderKind::Openai => llm(configure!(OpenAIResponsesClient::new(need_key()?, model))),
            ProviderKind::OpenaiCompatible => {
                if base.is_none() {
                    return Err(ApiError::BadRequest(
                        "openai_compatible providers need a base_url".into(),
                    ));
                }
                llm(configure!(OpenAIClient::new(
                    key.clone().unwrap_or_default(),
                    model
                )))
            }
            ProviderKind::Anthropic => llm(configure!(AnthropicClient::new(need_key()?, model))),
            ProviderKind::Gemini => llm(configure!(GeminiClient::new(need_key()?, model))),
        })
    }

    fn model_from(&self, built: Built) -> Arc<dyn ModelClient> {
        match built {
            Built::Mock => Arc::new(MockModel {
                word_delay: self.mock_word_delay,
            }),
            Built::Llm(p) => Arc::new(RetryingModel::new(
                Arc::new(*p),
                3,
                Duration::from_millis(500),
            )),
        }
    }

    /// The model client of a session (current provider record, or env keys for legacy ones).
    pub async fn session_model(
        &self,
        cfg: &SessionConfig,
    ) -> Result<Arc<dyn ModelClient>, ApiError> {
        let built = match &cfg.provider_id {
            Some(id) => {
                let rec = self.config.get_provider(id).await.map_err(|e| match e {
                    oort_agent::StoreError::NotFound(_) => {
                        ApiError::Conflict(format!("provider {id} of this session was deleted"))
                    }
                    other => other.into(),
                })?;
                if !rec.is_enabled {
                    return Err(ApiError::Conflict(format!(
                        "provider `{}` is disabled",
                        rec.name
                    )));
                }
                Self::build(&rec, &cfg.model)?
            }
            None => legacy_client(&cfg.provider, &cfg.model)?,
        };
        Ok(self.model_from(built))
    }

    /// Connectivity test: `list_models` (free), plus a 1-token chat when `ping` is set.
    pub async fn test_provider(&self, rec: &ProviderRecord, ping: bool) -> ProviderTest {
        let started = Instant::now();
        let model = Some(rec.default_model.trim().to_string()).filter(|m| !m.is_empty());
        let fail = |error: String, started: Instant| ProviderTest {
            ok: false,
            latency_ms: started.elapsed().as_millis() as u64,
            model_checked: model.clone(),
            model_found: None,
            models: vec![],
            error: Some(error),
        };
        let built = match Self::build(rec, model.as_deref().unwrap_or_default()) {
            Ok(b) => b,
            Err(e) => return fail(e.to_string(), started),
        };
        let client = match built {
            Built::Mock => {
                return ProviderTest {
                    ok: true,
                    latency_ms: started.elapsed().as_millis() as u64,
                    model_found: model.as_ref().map(|m| m == "mock"),
                    model_checked: model,
                    models: vec!["mock".into()],
                    error: None,
                };
            }
            Built::Llm(p) => *p,
        };
        let models = match client.list_models().await {
            Ok(mut m) => {
                m.sort();
                m
            }
            Err(e) => return fail(format!("list models: {e}"), started),
        };
        let model_found = model.as_ref().map(|m| {
            models
                .iter()
                .any(|x| x == m || x.strip_prefix("models/") == Some(m.as_str()))
        });
        if ping {
            let Some(m) = &model else {
                return fail("ping needs a default model".into(), started);
            };
            let mut req = ChatRequest::new(vec![Message::user("ping")]).with_model(m);
            req.max_tokens = Some(1);
            if let Err(e) = client.chat(req).await {
                return ProviderTest {
                    models,
                    model_found,
                    ..fail(format!("ping: {e}"), started)
                };
            }
        }
        ProviderTest {
            ok: true,
            latency_ms: started.elapsed().as_millis() as u64,
            model_checked: model,
            model_found,
            models,
            error: None,
        }
    }

    // ----- tools -----------------------------------------------------------------------

    /// Bindings (id + current name as prefix) for new sessions; every server must exist and
    /// be enabled.
    pub async fn bind_servers(&self, ids: &[String]) -> Result<Vec<McpBinding>, ApiError> {
        let mut out = Vec::new();
        for id in ids {
            if out.iter().any(|b: &McpBinding| &b.id == id) {
                continue;
            }
            let rec = self.config.get_mcp_server(id).await?;
            if !rec.is_enabled {
                return Err(ApiError::BadRequest(format!(
                    "MCP server `{}` is disabled",
                    rec.name
                )));
            }
            out.push(McpBinding {
                id: rec.id,
                prefix: rec.name,
            });
        }
        Ok(out)
    }

    /// The live executor for a session's tools. `strict` (creation) fails on any unavailable
    /// server; otherwise (resume) such servers are skipped and their tools answer with errors.
    pub async fn live_executor(
        self: &Arc<Self>,
        sel: &ToolSelection<'_>,
        strict: bool,
    ) -> Result<Arc<dyn ToolExecutor>, ApiError> {
        let mut b = CompositeToolExecutor::builder();
        // The legacy local tools (read_file, …) predate the builtin groups and share names
        // with them: a session uses one or the other.
        if sel.builtin && sel.groups.is_empty() {
            b = b.add(Namespace::None, self.builtin.clone());
        }
        let widgets = sel.widgets || sel.groups.contains(&Group::CoreUx);
        if widgets || !sel.skills.is_empty() {
            b = b.add(
                Namespace::None,
                Arc::new(session_tools::executor(
                    widgets,
                    sel.skills.clone(),
                    self.skills.clone(),
                )),
            );
        }
        if !sel.groups.is_empty() {
            let ctx = self.builtin_context(
                sel.workspace_root.clone(),
                sel.runtime.as_ref().map(|r| r.events.clone()),
            );
            let tools = oort_tools::executor(&sel.groups, &ctx).map_err(ApiError::BadRequest)?;
            b = b.add(Namespace::None, Arc::new(tools));
            if sel.groups.contains(&Group::Subagent) && sel.depth < self.subagents.max_depth {
                b = b.add(
                    Namespace::None,
                    Arc::new(oort_agent::LocalToolExecutor::new().with_tool(Arc::new(
                        SpawnSubagent {
                            registry: Arc::downgrade(self),
                            runtime: sel.runtime.clone(),
                        },
                    ))),
                );
            }
        }
        for binding in sel.mcp {
            let rec = match self.config.get_mcp_server(&binding.id).await {
                Ok(rec) if rec.is_enabled || !strict => rec,
                Ok(rec) => {
                    return Err(ApiError::BadRequest(format!(
                        "MCP server `{}` is disabled",
                        rec.name
                    )));
                }
                Err(e) if strict => return Err(e.into()),
                Err(e) => {
                    tracing::warn!(server = %binding.prefix, error = %e, "MCP server of session is gone");
                    continue;
                }
            };
            match self.pool.executor(&rec).await {
                Ok(ex) => b = b.add(Namespace::Prefix(binding.prefix.clone()), ex),
                Err(e) if strict => return Err(mcp_err(&rec.name, e)),
                Err(e) => {
                    tracing::warn!(server = %rec.name, error = %e, "MCP server unavailable; its tools will return errors")
                }
            }
        }
        Ok(Arc::new(
            b.build().map_err(|e| ApiError::BadRequest(e.to_string()))?,
        ))
    }

    /// Bindings of a pre-workbench session: the DB servers whose `{name}__` prefix appears in
    /// its frozen catalog (they came from `OORT_MCP_CONFIG`).
    async fn legacy_bindings(&self, state: &AgentState) -> Vec<McpBinding> {
        let tools = state.tool_snapshot().unwrap_or_default();
        let servers = self.config.list_mcp_servers().await.unwrap_or_default();
        servers
            .into_iter()
            .filter(|s| {
                let p = format!("{}__", s.name);
                tools.iter().any(|t| t.name.starts_with(&p))
            })
            .map(|s| McpBinding {
                id: s.id,
                prefix: s.name,
            })
            .collect()
    }

    /// One agent per session run. Tools are always the session's frozen catalog, served by
    /// what is live now ([`PinnedToolExecutor`]).
    #[allow(clippy::too_many_arguments)]
    pub async fn build_agent(
        self: &Arc<Self>,
        cfg: &SessionConfig,
        lineage: &Lineage,
        session_id: &str,
        state: &AgentState,
        store: Arc<dyn SessionStore>,
        approval: Arc<ApprovalHandle>,
        events: mpsc::Sender<AgentEvent>,
        cancel: CancellationToken,
    ) -> Result<Agent, ApiError> {
        let model = self.session_model(cfg).await?;
        let bindings = if cfg.is_legacy() {
            self.legacy_bindings(state).await
        } else {
            cfg.mcp_servers.clone()
        };
        let live = self
            .live_executor(
                &ToolSelection {
                    builtin: cfg.builtin_tools,
                    widgets: cfg.widgets,
                    skills: cfg.skills.iter().map(|s| s.name.clone()).collect(),
                    mcp: &bindings,
                    groups: parse_groups(&lineage.builtin_groups)?,
                    workspace_root: lineage.workspace_root.as_ref().map(Into::into),
                    runtime: Some(SubagentRuntime {
                        session_id: session_id.to_string(),
                        cfg: cfg.clone(),
                        lineage: lineage.clone(),
                        store: store.clone(),
                        events: events.clone(),
                    }),
                    depth: lineage.depth,
                },
                false,
            )
            .await?;
        let pinned = Arc::new(PinnedToolExecutor::new(
            state.tool_snapshot().unwrap_or_default(),
            live,
        ));
        let missing = pinned.missing();
        if !missing.is_empty() {
            tracing::warn!(session = %session_id, ?missing, "frozen tools not offered by any live server");
        }
        let mut b = Agent::builder(model.clone(), pinned)
            .context_manager(Arc::new(cfg.context_manager(session_id)))
            .hook(Arc::new(TracingHook))
            .hook(Arc::new(DynamicApproval(approval)))
            .events(events)
            .session_cancel(cancel);
        b = if cfg.provider == "mock" {
            b.compactor(Arc::new(RuleBasedCompactor {
                max_chars_per_message: 120,
            }))
        } else {
            b.compactor(Arc::new(LlmCompactor::new(model)))
        };
        // Persistence runs last so it records what the other hooks decided.
        b = b.hook(Arc::new(PersistenceHook::new(
            store,
            session_id.to_string(),
            state,
        )));
        b.build().map_err(|e| ApiError::Internal(e.to_string()))
    }

    // ----- startup -----------------------------------------------------------------------

    /// Imports `{"mcpServers": …}` entries whose names are not in the table yet.
    pub async fn import_mcp_config(&self, raw: &str) -> Result<Vec<String>, String> {
        let cfg = McpConfig::from_json(raw).map_err(|e| e.to_string())?;
        let existing: Vec<String> = self
            .config
            .list_mcp_servers()
            .await
            .map_err(|e| e.to_string())?
            .into_iter()
            .map(|s| s.name)
            .collect();
        let mut imported = Vec::new();
        for (name, entry) in cfg.servers {
            if existing.contains(&name) {
                continue;
            }
            let now = crate::now_ms();
            let mut rec = McpServerRecord {
                id: crate::new_id(),
                name: name.clone(),
                transport: oort_agent::McpTransportKind::Stdio,
                url: None,
                command: None,
                args: vec![],
                env: BTreeMap::new(),
                headers: BTreeMap::new(),
                auth: oort_agent::McpAuth::None,
                is_enabled: true,
                created_at: now,
                updated_at: now,
            };
            match entry {
                ServerEntry::Stdio(s) => {
                    rec.command = Some(s.command);
                    rec.args = s.args;
                    rec.env = s.env;
                }
                ServerEntry::Http {
                    url,
                    headers,
                    bearer_token,
                } => {
                    rec.transport = oort_agent::McpTransportKind::StreamableHttp;
                    rec.url = Some(url);
                    rec.headers = headers;
                    if let Some(token) = bearer_token {
                        rec.auth = oort_agent::McpAuth::Bearer { token };
                    }
                }
            }
            self.config
                .create_mcp_server(&rec)
                .await
                .map_err(|e| e.to_string())?;
            imported.push(name);
        }
        Ok(imported)
    }

    pub async fn close(&self) {
        self.pool.close_all().await;
    }
}

/// Pre-workbench sessions: kinds as they were then, keys from `*_API_KEY` / `*_BASE_URL`.
fn legacy_client(provider: &str, model: &str) -> Result<Built, ApiError> {
    let key = |name: &str| {
        std::env::var(name)
            .map_err(|_| ApiError::BadRequest(format!("{name} is not set on the server")))
    };
    let base = |name: &str| std::env::var(name).ok().filter(|v| !v.is_empty());
    macro_rules! with_base {
        ($c:expr, $var:literal) => {{
            let mut c = $c;
            if let Some(b) = base($var) {
                c = c.with_base_url(b);
            }
            c
        }};
    }
    Ok(match provider {
        "mock" => Built::Mock,
        "anthropic" => llm(with_base!(
            AnthropicClient::new(key("ANTHROPIC_API_KEY")?, model),
            "ANTHROPIC_BASE_URL"
        )),
        "openai" => llm(with_base!(
            OpenAIClient::new(key("OPENAI_API_KEY")?, model),
            "OPENAI_BASE_URL"
        )),
        "openai_responses" => llm(with_base!(
            OpenAIResponsesClient::new(key("OPENAI_API_KEY")?, model),
            "OPENAI_BASE_URL"
        )),
        "gemini" => llm(with_base!(
            GeminiClient::new(key("GEMINI_API_KEY")?, model),
            "GEMINI_BASE_URL"
        )),
        other => {
            return Err(ApiError::Internal(format!(
                "unknown legacy provider {other:?}"
            )));
        }
    })
}
