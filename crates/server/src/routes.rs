//! HTTP routes.

use std::convert::Infallible;
use std::path::Path as FsPath;
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::Router;
use axum::body::Bytes;
use axum::extract::{Json, Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::routing::{get, post};
use futures_util::{Stream, StreamExt};
use oort_agent::{McpAuth, McpServerRecord, PresetRecord, ProviderRecord};
use oort_mcp::{CallToolResult, McpError};
use serde::de::DeserializeOwned;
use tokio::sync::broadcast::error::RecvError;
use tokio_util::sync::CancellationToken;
use tower_http::services::{ServeDir, ServeFile};
use tower_http::trace::TraceLayer;

use crate::api::{
    ApiError, CreateSessionRequest, Envelope, InputRequest, PatchSessionRequest, SessionDetail,
    SessionExport, SessionSummary,
};
use crate::config_api::{
    McpServerInput, McpServerView, McpTestResponse, PresetInput, ProviderInput, ProviderView,
    ServerInfoView, SkillInput, SkillView, SkillsReload, ToolCallRequest, ToolCallResponse,
    ToolInfo,
};
use crate::manager::SessionManager;
use crate::oauth::{OAuthStart, OAuthView, callback_page};
use crate::registry::ProviderTest;

type AppState = Arc<SessionManager>;
type ApiResult<T> = Result<Json<T>, ApiError>;

pub fn router(manager: Arc<SessionManager>, web_dist: Option<&FsPath>) -> Router {
    let api = Router::new()
        .route("/sessions", get(list_sessions).post(create_session))
        .route(
            "/sessions/{id}",
            get(get_session).patch(patch_session).delete(delete_session),
        )
        .route("/sessions/{id}/events", get(session_events))
        .route("/sessions/{id}/input", post(session_input))
        .route("/sessions/{id}/cancel", post(cancel_session))
        .route("/sessions/{id}/retry", post(retry_session))
        .route("/sessions/{id}/export", get(export_session))
        .route("/sessions/{id}/children", get(session_children))
        .route("/commands", get(list_commands))
        .route("/commands/run", post(run_command))
        .route("/memories", get(search_memories).post(create_memory))
        .route(
            "/memories/{id}",
            get(get_memory).put(update_memory).delete(delete_memory),
        )
        .route("/providers", get(list_providers).post(create_provider))
        .route("/providers/test", post(test_new_provider))
        .route("/providers/models", post(models_new_provider))
        .route(
            "/providers/{id}",
            get(get_provider)
                .put(update_provider)
                .delete(delete_provider),
        )
        .route("/providers/{id}/test", post(test_provider))
        .route("/providers/{id}/models", post(provider_models))
        .route("/mcp-servers", get(list_mcp).post(create_mcp))
        .route("/mcp-servers/test", post(test_new_mcp))
        .route(
            "/mcp-servers/{id}",
            get(get_mcp).put(update_mcp).delete(delete_mcp),
        )
        .route("/mcp-servers/{id}/test", post(test_mcp))
        .route("/mcp-servers/{id}/tools/call", post(call_mcp_tool))
        .route("/presets", get(list_presets).post(create_preset))
        .route(
            "/presets/{id}",
            get(get_preset).put(update_preset).delete(delete_preset),
        )
        .route("/mcp-servers/{id}/oauth", get(oauth_status))
        .route("/mcp-servers/{id}/oauth/start", post(oauth_start))
        .route("/mcp-servers/{id}/oauth/disconnect", post(oauth_disconnect))
        .route("/mcp/oauth/callback", get(oauth_callback))
        .route("/skills", get(list_skills).post(create_skill))
        .route("/skills/reload", post(reload_skills))
        .route(
            "/skills/{id}",
            get(get_skill).put(update_skill).delete(delete_skill),
        );
    let mut app = Router::new()
        .nest("/api", api)
        .with_state(manager)
        .layer(TraceLayer::new_for_http());
    if let Some(dist) = web_dist {
        let index = dist.join("index.html");
        app = app.fallback_service(ServeDir::new(dist).fallback(ServeFile::new(index)));
    }
    app
}

/// An optional JSON body (empty body → `T::default()`).
fn optional_body<T: DeserializeOwned + Default>(body: &Bytes) -> Result<T, ApiError> {
    if body.iter().all(u8::is_ascii_whitespace) {
        return Ok(T::default());
    }
    serde_json::from_slice(body).map_err(|e| ApiError::BadRequest(format!("invalid JSON: {e}")))
}

// ----- sessions ------------------------------------------------------------------------------

async fn list_sessions(State(m): State<AppState>) -> ApiResult<Vec<SessionSummary>> {
    Ok(Json(m.list().await?))
}

async fn create_session(
    State(m): State<AppState>,
    Json(req): Json<CreateSessionRequest>,
) -> ApiResult<SessionDetail> {
    let id = m.create(req).await?;
    Ok(Json(m.detail(&id).await?))
}

async fn get_session(
    State(m): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<SessionDetail> {
    Ok(Json(m.detail(&id).await?))
}

async fn patch_session(
    State(m): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<PatchSessionRequest>,
) -> ApiResult<SessionDetail> {
    m.patch(&id, req).await?;
    Ok(Json(m.detail(&id).await?))
}

async fn delete_session(
    State(m): State<AppState>,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    m.delete(&id).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn export_session(
    State(m): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<SessionExport> {
    Ok(Json(m.export(&id).await?))
}

async fn session_input(
    State(m): State<AppState>,
    Path(id): Path<String>,
    Json(input): Json<InputRequest>,
) -> Result<StatusCode, ApiError> {
    m.input(&id, input).await?;
    Ok(StatusCode::ACCEPTED)
}

async fn retry_session(
    State(m): State<AppState>,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    m.retry(&id).await?;
    Ok(StatusCode::ACCEPTED)
}

async fn cancel_session(
    State(m): State<AppState>,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    Ok(if m.cancel(&id).await? {
        StatusCode::ACCEPTED
    } else {
        StatusCode::NO_CONTENT
    })
}

fn sse_event(env: &Envelope, boot: &str) -> Event {
    let mut ev = Event::default().json_data(env).unwrap_or_default();
    if let Some(seq) = env.seq() {
        ev = ev.id(format!("{boot}:{seq}"));
    }
    ev
}

/// `hello`, then missed events (when `Last-Event-ID` could be resumed), then live events.
/// `data:` is an [`Envelope`] JSON; `id:` is `{boot}:{seq}`.
async fn session_events(
    State(m): State<AppState>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Result<Sse<impl Stream<Item = Result<Event, Infallible>>>, ApiError> {
    let last = headers.get("last-event-id").and_then(|v| v.to_str().ok());
    let stream = m.subscribe(&id, last).await?;
    let boot = m.boot().to_string();
    let closing: CancellationToken = m.closing();

    let head: Vec<Envelope> = std::iter::once(stream.hello).chain(stream.replay).collect();
    let tail = futures_util::stream::unfold(Some(stream.rx), |rx| async move {
        let mut rx = rx?;
        let env = match rx.recv().await {
            Ok(env) => env,
            Err(RecvError::Lagged(skipped)) => Envelope::Lagged { skipped },
            Err(RecvError::Closed) => return None,
        };
        // A deleted session's stream ends after telling the client.
        let next = (!matches!(env, Envelope::Deleted { .. })).then_some(rx);
        Some((env, next))
    });
    let events = futures_util::stream::iter(head)
        .chain(tail)
        .map(move |env| Ok(sse_event(&env, &boot)))
        // Long-lived streams would otherwise block the server's graceful shutdown forever.
        .take_until(closing.cancelled_owned());
    Ok(Sse::new(events).keep_alive(KeepAlive::default()))
}

// ----- providers -----------------------------------------------------------------------------

async fn list_providers(State(m): State<AppState>) -> ApiResult<Vec<ProviderView>> {
    let list = m.registry().config.list_providers().await?;
    Ok(Json(list.iter().map(ProviderView::from).collect()))
}

async fn get_provider(
    State(m): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<ProviderView> {
    Ok(Json((&m.registry().config.get_provider(&id).await?).into()))
}

async fn create_provider(
    State(m): State<AppState>,
    Json(input): Json<ProviderInput>,
) -> ApiResult<ProviderView> {
    let rec = input.create()?;
    m.registry().config.create_provider(&rec).await?;
    Ok(Json((&rec).into()))
}

async fn update_provider(
    State(m): State<AppState>,
    Path(id): Path<String>,
    Json(input): Json<ProviderInput>,
) -> ApiResult<ProviderView> {
    let cfg = &m.registry().config;
    let rec = input.apply(&cfg.get_provider(&id).await?)?;
    cfg.update_provider(&rec).await?;
    Ok(Json((&rec).into()))
}

/// Sessions created with a deleted provider stay readable but cannot run again.
async fn delete_provider(
    State(m): State<AppState>,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    m.registry().config.delete_provider(&id).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Tests the stored provider, optionally with unsaved changes from the body.
async fn test_provider(
    State(m): State<AppState>,
    Path(id): Path<String>,
    body: Bytes,
) -> ApiResult<ProviderTest> {
    let input: ProviderInput = optional_body(&body)?;
    let ping = input.ping;
    let rec = input.apply(&m.registry().config.get_provider(&id).await?)?;
    Ok(Json(m.registry().test_provider(&rec, ping).await))
}

#[derive(serde::Serialize)]
struct ModelList {
    models: Vec<String>,
}

/// Model ids of a saved provider (an optional body overrides unsaved fields such as the key).
async fn provider_models(
    State(m): State<AppState>,
    Path(id): Path<String>,
    body: Bytes,
) -> ApiResult<ModelList> {
    let input: ProviderInput = optional_body(&body)?;
    let rec = input.apply(&m.registry().config.get_provider(&id).await?)?;
    Ok(Json(ModelList {
        models: m.registry().list_models(&rec).await?,
    }))
}

/// Model ids for an unsaved provider form.
async fn models_new_provider(
    State(m): State<AppState>,
    Json(mut input): Json<ProviderInput>,
) -> ApiResult<ModelList> {
    // Listing needs no name; give the throwaway record one so validation passes.
    if input.name.as_deref().is_none_or(|n| n.trim().is_empty()) {
        input.name = Some("unsaved".into());
    }
    let rec: ProviderRecord = input.create()?;
    Ok(Json(ModelList {
        models: m.registry().list_models(&rec).await?,
    }))
}

/// Tests an unsaved provider form.
async fn test_new_provider(
    State(m): State<AppState>,
    Json(input): Json<ProviderInput>,
) -> ApiResult<ProviderTest> {
    let ping = input.ping;
    let rec: ProviderRecord = input.create()?;
    Ok(Json(m.registry().test_provider(&rec, ping).await))
}

// ----- MCP servers ---------------------------------------------------------------------------

/// The view plus the OAuth connection state of `oauth` servers.
async fn mcp_view(m: &SessionManager, rec: &McpServerRecord) -> Result<McpServerView, ApiError> {
    let mut view = McpServerView::from(rec);
    if matches!(rec.auth, McpAuth::OAuth { .. }) {
        view.oauth = m.registry().oauth.status(&rec.id).await?;
    }
    Ok(view)
}

async fn list_mcp(State(m): State<AppState>) -> ApiResult<Vec<McpServerView>> {
    let mut out = Vec::new();
    for rec in m.registry().config.list_mcp_servers().await? {
        out.push(mcp_view(&m, &rec).await?);
    }
    Ok(Json(out))
}

async fn get_mcp(State(m): State<AppState>, Path(id): Path<String>) -> ApiResult<McpServerView> {
    let rec = m.registry().config.get_mcp_server(&id).await?;
    Ok(Json(mcp_view(&m, &rec).await?))
}

async fn create_mcp(
    State(m): State<AppState>,
    Json(input): Json<McpServerInput>,
) -> ApiResult<McpServerView> {
    let rec = input.create()?;
    m.registry().config.create_mcp_server(&rec).await?;
    Ok(Json((&rec).into()))
}

/// New sessions use the new settings; existing sessions keep their frozen tool catalog.
async fn update_mcp(
    State(m): State<AppState>,
    Path(id): Path<String>,
    Json(input): Json<McpServerInput>,
) -> ApiResult<McpServerView> {
    let reg = m.registry();
    let rec = input.apply(&reg.config.get_mcp_server(&id).await?)?;
    reg.config.update_mcp_server(&rec).await?;
    reg.pool.evict(&id).await;
    Ok(Json((&rec).into()))
}

async fn delete_mcp(
    State(m): State<AppState>,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    let reg = m.registry();
    reg.config.delete_mcp_server(&id).await?;
    reg.pool.evict(&id).await;
    Ok(StatusCode::NO_CONTENT)
}

async fn probe(m: &SessionManager, rec: &McpServerRecord) -> McpTestResponse {
    let started = Instant::now();
    let mut opts = m.registry().pool.options().clone();
    opts.request_timeout = Duration::from_secs(15);
    let tokens = m.registry().pool.token_provider(rec);
    match oort_mcp::probe(rec, opts, tokens).await {
        Ok(p) => McpTestResponse {
            ok: true,
            latency_ms: p.latency_ms,
            server_info: Some(ServerInfoView {
                name: p.server.server_info.name,
                version: p.server.server_info.version,
                protocol_version: p.server.protocol_version,
                instructions: p.server.instructions,
            }),
            tools: p
                .tools
                .iter()
                .map(|t| ToolInfo {
                    read_only: t.is_read_only(),
                    name: t.name.clone(),
                    title: t.title.clone(),
                    description: t.description.clone(),
                    input_schema: t.input_schema.clone(),
                })
                .collect(),
            error: None,
        },
        Err(e) => McpTestResponse {
            ok: false,
            latency_ms: started.elapsed().as_millis() as u64,
            server_info: None,
            tools: vec![],
            error: Some(e.to_string()),
        },
    }
}

/// Fresh connection: `initialize` + `tools/list`, then disconnect.
async fn test_mcp(
    State(m): State<AppState>,
    Path(id): Path<String>,
    body: Bytes,
) -> ApiResult<McpTestResponse> {
    let input: McpServerInput = optional_body(&body)?;
    let reg = m.registry();
    let rec = input.apply(&reg.config.get_mcp_server(&id).await?)?;
    let res = probe(&m, &rec).await;
    if res.ok {
        // New sessions and the playground pick up the catalog just discovered.
        reg.pool.evict(&id).await;
    }
    Ok(Json(res))
}

async fn test_new_mcp(
    State(m): State<AppState>,
    Json(input): Json<McpServerInput>,
) -> ApiResult<McpTestResponse> {
    let rec = input.create()?;
    Ok(Json(probe(&m, &rec).await))
}

/// Tool playground: calls one tool directly (no LLM, no approval).
async fn call_mcp_tool(
    State(m): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<ToolCallRequest>,
) -> ApiResult<ToolCallResponse> {
    let reg = m.registry();
    let rec = reg.config.get_mcp_server(&id).await?;
    let started = Instant::now();
    let elapsed = || started.elapsed().as_millis() as u64;
    let fail = |error: String, latency_ms: u64| ToolCallResponse {
        ok: false,
        latency_ms,
        is_error: true,
        content: serde_json::Value::Null,
        structured_content: None,
        text: None,
        error: Some(error),
    };
    let executor = match reg.pool.executor(&rec).await {
        Ok(ex) => ex,
        Err(e) => return Ok(Json(fail(format!("connect: {e}"), elapsed()))),
    };
    let client = executor.client();
    let cancel = CancellationToken::new();
    let call = client.call_tool_raw(&req.tool_name, req.arguments, &cancel);
    let result = match req.timeout_ms {
        Some(ms) => tokio::time::timeout(Duration::from_millis(ms), call)
            .await
            .unwrap_or(Err(McpError::Transport(oort_mcp::TransportError::Timeout))),
        None => call.await,
    };
    Ok(Json(match result {
        Ok(raw) => {
            let parsed: Option<CallToolResult> = serde_json::from_value(raw.clone()).ok();
            ToolCallResponse {
                ok: true,
                latency_ms: elapsed(),
                is_error: parsed.as_ref().is_some_and(|r| r.is_error),
                content: raw.get("content").cloned().unwrap_or_default(),
                structured_content: raw.get("structuredContent").cloned(),
                text: parsed.as_ref().map(CallToolResult::to_text),
                error: None,
            }
        }
        Err(e) => {
            if matches!(e, McpError::Transport(_)) {
                reg.pool.evict(&id).await;
            }
            fail(e.to_string(), elapsed())
        }
    }))
}

// ----- presets -------------------------------------------------------------------------------

async fn check_preset_refs(m: &SessionManager, p: &PresetRecord) -> Result<(), ApiError> {
    let cfg = &m.registry().config;
    m.registry()
        .skills
        .resolve(&p.skill_ids)
        .await
        .map_err(|e| ApiError::BadRequest(format!("skills: {e}")))?;
    cfg.get_provider(&p.provider_id)
        .await
        .map_err(|_| ApiError::BadRequest(format!("unknown provider {}", p.provider_id)))?;
    for id in &p.mcp_server_ids {
        cfg.get_mcp_server(id)
            .await
            .map_err(|_| ApiError::BadRequest(format!("unknown MCP server {id}")))?;
    }
    Ok(())
}

async fn list_presets(State(m): State<AppState>) -> ApiResult<Vec<PresetRecord>> {
    Ok(Json(m.registry().config.list_presets().await?))
}

async fn get_preset(State(m): State<AppState>, Path(id): Path<String>) -> ApiResult<PresetRecord> {
    Ok(Json(m.registry().config.get_preset(&id).await?))
}

async fn create_preset(
    State(m): State<AppState>,
    Json(input): Json<PresetInput>,
) -> ApiResult<PresetRecord> {
    let rec = input.create()?;
    check_preset_refs(&m, &rec).await?;
    m.registry().config.create_preset(&rec).await?;
    Ok(Json(rec))
}

async fn update_preset(
    State(m): State<AppState>,
    Path(id): Path<String>,
    Json(input): Json<PresetInput>,
) -> ApiResult<PresetRecord> {
    let cfg = &m.registry().config;
    let rec = input.apply(&cfg.get_preset(&id).await?)?;
    check_preset_refs(&m, &rec).await?;
    cfg.update_preset(&rec).await?;
    Ok(Json(rec))
}

async fn delete_preset(
    State(m): State<AppState>,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    m.registry().config.delete_preset(&id).await?;
    Ok(StatusCode::NO_CONTENT)
}

// ----- MCP OAuth -----------------------------------------------------------------------------

async fn oauth_status(
    State(m): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Option<OAuthView>> {
    m.registry().config.get_mcp_server(&id).await?;
    Ok(Json(m.registry().oauth.status(&id).await?))
}

/// Discovery + client registration + PKCE; the UI opens `authorization_url` in a popup.
async fn oauth_start(State(m): State<AppState>, Path(id): Path<String>) -> ApiResult<OAuthStart> {
    let rec = m.registry().config.get_mcp_server(&id).await?;
    Ok(Json(m.registry().oauth.start(&rec).await?))
}

#[derive(serde::Deserialize)]
struct CallbackQuery {
    code: Option<String>,
    state: Option<String>,
    error: Option<String>,
    error_description: Option<String>,
}

/// Redirect target of the authorization server. Always answers with a small HTML page that
/// reports the outcome to the opener window.
async fn oauth_callback(
    State(m): State<AppState>,
    axum::extract::Query(q): axum::extract::Query<CallbackQuery>,
) -> axum::response::Html<String> {
    let flows = &m.registry().oauth;
    let server = match &q.state {
        Some(s) => flows.pending_server(s).await,
        None => None,
    };
    if let Some(err) = q.error {
        let msg = q
            .error_description
            .map(|d| format!("{err}: {d}"))
            .unwrap_or(err);
        return axum::response::Html(callback_page(server.as_deref(), Some(&msg)));
    }
    let (Some(code), Some(state)) = (q.code, q.state) else {
        return axum::response::Html(callback_page(None, Some("missing code or state")));
    };
    match flows.callback(&code, &state).await {
        Ok(server_id) => {
            // New connections pick up the token; existing ones refresh through the provider.
            m.registry().pool.evict(&server_id).await;
            axum::response::Html(callback_page(Some(&server_id), None))
        }
        Err(e) => axum::response::Html(callback_page(server.as_deref(), Some(&e.to_string()))),
    }
}

async fn oauth_disconnect(
    State(m): State<AppState>,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    m.registry().config.get_mcp_server(&id).await?;
    m.registry().oauth.disconnect(&id).await?;
    m.registry().pool.evict(&id).await;
    Ok(StatusCode::NO_CONTENT)
}

// ----- skills --------------------------------------------------------------------------------

async fn list_skills(State(m): State<AppState>) -> ApiResult<Vec<SkillView>> {
    let list = m.registry().skills.list().await?;
    Ok(Json(list.into_iter().map(SkillView::from).collect()))
}

async fn get_skill(State(m): State<AppState>, Path(id): Path<String>) -> ApiResult<SkillView> {
    Ok(Json(m.registry().skills.get(&id).await?.into()))
}

fn file_skill_readonly(id: &str) -> Result<(), ApiError> {
    if id.starts_with(oort_skills::FILE_PREFIX) {
        return Err(ApiError::Conflict(
            "file skills are read-only here: edit the SKILL.md file, or copy it into a new skill"
                .into(),
        ));
    }
    Ok(())
}

async fn create_skill(
    State(m): State<AppState>,
    Json(input): Json<SkillInput>,
) -> ApiResult<SkillView> {
    let rec = input.create()?;
    m.registry().skills.store().create_skill(&rec).await?;
    Ok(Json(oort_skills::Skill::from(rec).into()))
}

async fn update_skill(
    State(m): State<AppState>,
    Path(id): Path<String>,
    Json(input): Json<SkillInput>,
) -> ApiResult<SkillView> {
    file_skill_readonly(&id)?;
    let store = m.registry().skills.store();
    let rec = input.apply(&store.get_skill(&id).await?)?;
    store.update_skill(&rec).await?;
    Ok(Json(oort_skills::Skill::from(rec).into()))
}

/// Sessions keep their frozen index; `load_skill` then reports that the skill is gone.
async fn delete_skill(
    State(m): State<AppState>,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    file_skill_readonly(&id)?;
    m.registry().skills.store().delete_skill(&id).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn reload_skills(State(m): State<AppState>) -> ApiResult<SkillsReload> {
    let skills = &m.registry().skills;
    let (file_skills, warnings) = skills.reload().await;
    Ok(Json(SkillsReload {
        file_skills,
        warnings,
        dirs: skills
            .dirs()
            .iter()
            .map(|d| d.display().to_string())
            .collect(),
    }))
}

// ----- subagents, commands, memories ---------------------------------------------------------

async fn session_children(
    State(m): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Vec<SessionSummary>> {
    Ok(Json(m.children(&id).await?))
}

async fn list_commands(State(m): State<AppState>) -> ApiResult<Vec<crate::commands::CommandInfo>> {
    Ok(Json(crate::commands::list(&m).await?))
}

async fn run_command(
    State(m): State<AppState>,
    Json(req): Json<crate::commands::RunCommand>,
) -> ApiResult<crate::commands::CommandResult> {
    Ok(Json(crate::commands::run(&m, req).await?))
}

#[derive(serde::Deserialize, Default)]
struct MemoryParams {
    #[serde(default)]
    q: Option<String>,
    #[serde(default)]
    tag: Option<String>,
    #[serde(default)]
    scope: Option<String>,
    #[serde(default)]
    limit: Option<u32>,
}

async fn search_memories(
    State(m): State<AppState>,
    axum::extract::Query(p): axum::extract::Query<MemoryParams>,
) -> ApiResult<Vec<oort_agent::MemoryHit>> {
    let q = oort_agent::MemoryQuery {
        text: p.q.unwrap_or_default(),
        tags: p.tag.into_iter().filter(|t| !t.is_empty()).collect(),
        scope: p.scope.filter(|s| !s.is_empty()),
        limit: Some(p.limit.unwrap_or(100)),
    };
    Ok(Json(m.registry().memories.search_memories(&q).await?))
}

#[derive(serde::Deserialize, Default)]
struct MemoryInput {
    title: Option<String>,
    content: Option<String>,
    tags: Option<Vec<String>>,
    scope: Option<String>,
}

impl MemoryInput {
    fn apply(self, cur: oort_agent::MemoryRecord) -> Result<oort_agent::MemoryRecord, ApiError> {
        let rec = oort_agent::MemoryRecord {
            title: self
                .title
                .map(|t| t.trim().to_string())
                .unwrap_or(cur.title),
            content: self.content.unwrap_or(cur.content),
            tags: self.tags.unwrap_or(cur.tags),
            scope: self
                .scope
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .unwrap_or(cur.scope),
            updated_at: crate::now_ms(),
            ..cur
        };
        if rec.title.is_empty() || rec.content.trim().is_empty() {
            return Err(ApiError::BadRequest(
                "`title` and `content` are required".into(),
            ));
        }
        Ok(rec)
    }
}

async fn get_memory(
    State(m): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<oort_agent::MemoryRecord> {
    Ok(Json(m.registry().memories.get_memory(&id).await?))
}

async fn create_memory(
    State(m): State<AppState>,
    Json(input): Json<MemoryInput>,
) -> ApiResult<oort_agent::MemoryRecord> {
    let now = crate::now_ms();
    let rec = input.apply(oort_agent::MemoryRecord {
        id: crate::new_id(),
        title: String::new(),
        content: String::new(),
        tags: vec![],
        scope: "global".into(),
        created_at: now,
        updated_at: now,
    })?;
    m.registry().memories.create_memory(&rec).await?;
    Ok(Json(rec))
}

async fn update_memory(
    State(m): State<AppState>,
    Path(id): Path<String>,
    Json(input): Json<MemoryInput>,
) -> ApiResult<oort_agent::MemoryRecord> {
    let store = &m.registry().memories;
    let rec = input.apply(store.get_memory(&id).await?)?;
    store.update_memory(&rec).await?;
    Ok(Json(rec))
}

async fn delete_memory(
    State(m): State<AppState>,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    m.registry().memories.delete_memory(&id).await?;
    Ok(StatusCode::NO_CONTENT)
}
