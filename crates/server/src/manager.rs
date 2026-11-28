//! Runs one background `run_loop` per active session and fans its events out to SSE clients.
//!
//! ```text
//!  POST /input ──► inbox (mpsc) ──► run_loop ──► AgentEvent (mpsc) ──► forwarder ──► Hub ──► SSE
//!                                     │ after_step                                  (replay buffer,
//!                                     ▼                                              live turn)
//!                               SQLite (PersistenceHook) ◄── GET /sessions/:id (snapshot)
//! ```
//!
//! Sessions are activated lazily (on create, input, or when a `running` session is watched),
//! loaded from the store, and removed from the active map when their loop exits.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Weak};
use std::time::Duration;

use oort_agent::{
    AgentEvent, AgentState, FrozenToolset, Inbox, LoopInput, NewSession, SessionStatus,
    SessionStore,
};
use oort_llm::{ContentPart, Message};
use tokio::sync::{Mutex, broadcast, mpsc};
use tokio_util::sync::CancellationToken;

use crate::api::InvokedSkill;
use crate::api::{
    ApiError, CheckpointView, CreateSessionRequest, Envelope, InputRequest, LoopError,
    PatchSessionRequest, ProviderSummary, RawMessage, SessionDetail, SessionExport, SessionSummary,
};
use crate::approval::ApprovalHandle;
use crate::config::{SessionConfig, SkillBinding};
use crate::hub::Hub;
use crate::registry::{Registry, ToolSelection, parse_groups};
use crate::session_tools::RENDER_WIDGET;
use crate::subagent::{ContextMode, EventFn, SubagentRuntime, SubagentTask, run_subagent};
use oort_agent::{Lineage, SkillExecutionMode};
use oort_tools::Group;

/// Handles to a running loop.
struct ActiveSession {
    inbox: mpsc::Sender<LoopInput>,
    cancel: CancellationToken,
    approval: Arc<ApprovalHandle>,
    /// Read by the context manager on every request.
    thinking: oort_agent::LiveThinking,
    /// Cancelled when the loop task has fully exited.
    finished: CancellationToken,
    generation: u64,
}

/// Everything an SSE connection starts with.
pub struct EventStream {
    pub hello: Envelope,
    pub replay: Vec<Envelope>,
    pub rx: broadcast::Receiver<Envelope>,
}

pub struct SessionManager {
    store: Arc<dyn SessionStore>,
    registry: Arc<Registry>,
    active: Arc<Mutex<BTreeMap<String, ActiveSession>>>,
    hubs: Mutex<BTreeMap<String, Arc<Hub>>>,
    generation: AtomicU64,
    /// Identifies this process in SSE ids: sequence numbers restart with the process.
    boot: String,
    /// Cancelled by [`SessionManager::shutdown`]; ends open SSE streams so the HTTP server
    /// can finish its graceful shutdown.
    closing: CancellationToken,
    /// Skill subagents running for a session (cancelled by stop / delete / shutdown).
    skill_runs: Mutex<BTreeMap<String, Vec<CancellationToken>>>,
    /// The error the last loop of a session exited with (until the loop runs again).
    errors: Arc<Mutex<BTreeMap<String, LoopError>>>,
    /// Automatic retries made since the last success or user action, per session.
    retries: Arc<Mutex<BTreeMap<String, u32>>>,
    me: Weak<SessionManager>,
}

impl SessionManager {
    pub fn new(store: Arc<dyn SessionStore>, registry: Arc<Registry>) -> Arc<Self> {
        Arc::new_cyclic(|me| Self {
            store,
            registry,
            active: Arc::default(),
            hubs: Mutex::default(),
            generation: AtomicU64::new(0),
            boot: crate::new_id(),
            closing: CancellationToken::new(),
            skill_runs: Mutex::default(),
            errors: Arc::default(),
            retries: Arc::default(),
            me: me.clone(),
        })
    }

    pub fn registry(&self) -> &Arc<Registry> {
        &self.registry
    }

    pub fn boot(&self) -> &str {
        &self.boot
    }

    pub async fn create(&self, req: CreateSessionRequest) -> Result<String, ApiError> {
        let cfgs = &self.registry.config;
        let preset = match &req.preset_id {
            Some(id) => Some(cfgs.get_preset(id).await?),
            None => None,
        };
        let provider_id = req
            .provider_id
            .clone()
            .or_else(|| preset.as_ref().map(|p| p.provider_id.clone()))
            .ok_or_else(|| {
                ApiError::BadRequest("`provider_id` or `preset_id` is required".into())
            })?;
        let provider = cfgs.get_provider(&provider_id).await.map_err(|e| match e {
            oort_agent::StoreError::NotFound(_) => {
                ApiError::BadRequest(format!("unknown provider {provider_id}"))
            }
            other => other.into(),
        })?;
        if !provider.is_enabled {
            return Err(ApiError::BadRequest(format!(
                "provider `{}` is disabled",
                provider.name
            )));
        }
        let nonempty =
            |s: Option<String>| s.map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
        let model = nonempty(req.model.clone())
            .or_else(|| nonempty(preset.as_ref().map(|p| p.model.clone())))
            .or_else(|| nonempty(Some(provider.default_model.clone())))
            .ok_or_else(|| ApiError::BadRequest("model must not be empty".into()))?;
        let server_ids = req
            .mcp_server_ids
            .clone()
            .or_else(|| preset.as_ref().map(|p| p.mcp_server_ids.clone()))
            .unwrap_or_default();
        let builtin = req
            .builtin_tools
            .or(preset.as_ref().map(|p| p.builtin_tools))
            .unwrap_or(true);
        let system_prompt = req
            .system_prompt
            .clone()
            .or_else(|| preset.as_ref().map(|p| p.system_prompt.clone()))
            .unwrap_or_default();
        let approval_policy = req
            .approval_policy
            .or(preset.as_ref().map(|p| p.approval_policy))
            .unwrap_or_default();
        let reasoning_effort = req.reasoning_effort.unwrap_or_default().or(preset
            .as_ref()
            .map(|p| p.reasoning_effort)
            .unwrap_or_default());

        // Fail fast on unusable keys and unreachable servers, before anything is stored.
        let widgets = req.widgets.unwrap_or(true);
        let skill_ids = req
            .skill_ids
            .clone()
            .or_else(|| preset.as_ref().map(|p| p.skill_ids.clone()))
            .unwrap_or_default();
        let skills = self
            .registry
            .skills
            .resolve(&skill_ids)
            .await
            .map_err(|e| match e {
                oort_agent::StoreError::NotFound(id) => {
                    ApiError::BadRequest(format!("unknown skill {id}"))
                }
                other => other.into(),
            })?;

        let group_names = req
            .builtin_groups
            .clone()
            .or_else(|| preset.as_ref().map(|p| p.builtin_groups.clone()))
            .unwrap_or_default();
        let groups = parse_groups(&group_names)?;
        let workspace_root = match (
            groups.contains(&Group::Workspace),
            req.workspace_root
                .clone()
                .or_else(|| preset.as_ref().and_then(|p| p.workspace_root.clone()))
                .filter(|r| !r.trim().is_empty()),
        ) {
            (true, Some(root)) => Some(canonical_workspace(&root)?),
            (true, None) => {
                return Err(ApiError::BadRequest(
                    "the workspace tool group needs a workspace_root".into(),
                ));
            }
            (false, _) => None,
        };

        Registry::check_provider(&provider, &model)?;
        let bindings = self.registry.bind_servers(&server_ids).await?;
        let live = self
            .registry
            .live_executor(
                &ToolSelection {
                    builtin,
                    widgets,
                    skills: skills.iter().map(|s| s.name.clone()).collect(),
                    mcp: &bindings,
                    groups: groups.clone(),
                    workspace_root: workspace_root.as_ref().map(Into::into),
                    runtime: None,
                    depth: 0,
                },
                true,
            )
            .await?;
        // The only time a catalog is frozen: sorted now, never derived again.
        let frozen = FrozenToolset::freeze(live.as_ref())
            .map_err(|e| ApiError::BadRequest(e.to_string()))?;

        let cfg = SessionConfig {
            provider: provider.kind.as_str().into(),
            provider_id: Some(provider.id.clone()),
            model: model.clone(),
            max_context_tokens: req
                .max_context_tokens
                .or(provider.defaults.context_window)
                .unwrap_or(200_000),
            max_tokens: req.max_tokens,
            builtin_tools: builtin,
            mcp_servers: bindings,
            approval: None,
            widgets,
            skills: skills
                .iter()
                .map(|s| SkillBinding {
                    id: s.id.clone(),
                    name: s.name.clone(),
                })
                .collect(),
        };
        // Progressive disclosure: only the sorted name/description index is frozen into the
        // prefix; bodies come later through `load_skill` or `/name`.
        let system_prompt = if skills.is_empty() {
            system_prompt
        } else {
            let index = oort_skills::index_text(&skills);
            if system_prompt.trim().is_empty() {
                index
            } else {
                format!("{system_prompt}\n\n{index}")
            }
        };
        let initial: Vec<Message> = req
            .message
            .filter(|m| !m.trim().is_empty())
            .map(Message::user)
            .into_iter()
            .collect();
        let state = AgentState::with_tools(system_prompt, initial, &frozen);
        let title = req
            .title
            .or_else(|| preset.as_ref().map(|p| p.name.clone()))
            .unwrap_or_default();
        let meta = NewSession {
            id: None,
            title,
            provider: cfg.provider.clone(),
            model,
            config: serde_json::to_value(&cfg).map_err(|e| ApiError::Internal(e.to_string()))?,
            approval_policy,
            lineage: Lineage {
                workspace_root,
                builtin_groups: groups.iter().map(|g| g.as_str().to_string()).collect(),
                ..Default::default()
            },
            reasoning_effort,
        };
        let id = self.store.create_session(meta, &state).await?;
        self.ensure_active(&id).await?;
        Ok(id)
    }

    pub async fn list(&self) -> Result<Vec<SessionSummary>, ApiError> {
        let active = self.active.lock().await;
        let errors = self.errors.lock().await;
        Ok(self
            .store
            .list_sessions()
            .await?
            .into_iter()
            .map(|meta| SessionSummary {
                active: active.contains_key(&meta.id),
                failed: errors.contains_key(&meta.id),
                meta,
            })
            .collect())
    }

    /// Snapshot from the store (committed after every step, so it is always consistent).
    pub async fn detail(&self, id: &str) -> Result<SessionDetail, ApiError> {
        let stored = self.store.load_state(id).await?;
        let calls = self.store.list_llm_calls(id).await?;
        let cfg = settings(&stored.meta.config)?;
        let active = self.active.lock().await.contains_key(id);
        let root = stored.meta.lineage.workspace_root.clone();
        let effective = stored.meta.reasoning_effort.or(self
            .registry
            .provider_defaults(&cfg)
            .await
            .reasoning_effort);
        let mut detail = SessionDetail::build(
            stored.meta,
            active,
            cfg,
            &stored.state,
            &stored.message_usage,
            &calls,
        );
        detail.last_error = self.errors.lock().await.get(id).cloned();
        detail.effective_reasoning_effort = effective;
        // What each approval card shows: a diff, the command, the settings, …
        let ctx = self.registry.builtin_context(root.map(Into::into), None);
        for p in &mut detail.pending_approvals {
            p.preview = oort_tools::preview(&p.call.name, &p.call.arguments, &ctx).await;
        }
        Ok(detail)
    }

    /// Subagent sessions spawned by `id`.
    pub async fn children(&self, id: &str) -> Result<Vec<SessionSummary>, ApiError> {
        self.store.load_state(id).await?;
        let active = self.active.lock().await;
        let errors = self.errors.lock().await;
        Ok(self
            .store
            .list_children(id)
            .await?
            .into_iter()
            .map(|meta| SessionSummary {
                active: active.contains_key(&meta.id),
                failed: errors.contains_key(&meta.id),
                meta,
            })
            .collect())
    }

    pub async fn input(&self, id: &str, input: InputRequest) -> Result<(), ApiError> {
        self.retries.lock().await.remove(id);
        let input = match input {
            InputRequest::Steer {
                text,
                invoked_skills,
            } => match self.prepare_user_input(id, &text, invoked_skills).await? {
                Expanded::Compact => LoopInput::Compact,
                Expanded::Text(t) => LoopInput::Steer(Message::user(t)),
                Expanded::Deferred => return Ok(()),
            },
            InputRequest::Interrupt {
                text,
                invoked_skills,
            } => match self.prepare_user_input(id, &text, invoked_skills).await? {
                Expanded::Compact => LoopInput::Compact,
                Expanded::Text(t) => LoopInput::Interrupt(Message::user(t)),
                Expanded::Deferred => return Ok(()),
            },
            InputRequest::Approval { call_id, decision } => {
                LoopInput::Approval { call_id, decision }
            }
            InputRequest::Compact => LoopInput::Compact,
        };
        let inbox = self.ensure_active(id).await?;
        inbox
            .send(input)
            .await
            .map_err(|_| ApiError::Conflict("session loop is not running".into()))
    }

    /// Renames and/or switches the approval policy (applies to a running loop immediately).
    pub async fn patch(&self, id: &str, req: PatchSessionRequest) -> Result<(), ApiError> {
        if let Some(title) = &req.title {
            self.store.set_title(id, title.trim()).await?;
        }
        if let Some(policy) = req.approval_policy {
            self.store.set_approval_policy(id, policy).await?;
            if let Some(a) = self.active.lock().await.get(id) {
                a.approval.set(policy);
            }
        }
        if let Some(effort) = req.reasoning_effort {
            self.store.set_reasoning_effort(id, effort).await?;
            let cfg = settings(&self.store.load_state(id).await?.meta.config)?;
            let fallback = self.registry.provider_defaults(&cfg).await.reasoning_effort;
            if let Some(a) = self.active.lock().await.get(id) {
                *a.thinking.write().unwrap_or_else(|e| e.into_inner()) =
                    effort.or(fallback).thinking();
            }
        }
        let meta = self.store.load_state(id).await?.meta;
        self.hub(id).await.publish(|seq| Envelope::SessionUpdated {
            seq,
            title: meta.title,
            approval_policy: meta.approval_policy,
            reasoning_effort: meta.reasoning_effort,
        });
        Ok(())
    }

    /// Cancels the loop (waiting for it to exit), then deletes the session and its history.
    pub async fn delete(&self, id: &str) -> Result<(), ApiError> {
        self.store.load_state(id).await?;
        let finished = self.active.lock().await.get(id).map(|a| {
            a.cancel.cancel();
            a.finished.clone()
        });
        if let Some(f) = finished
            && tokio::time::timeout(Duration::from_secs(5), f.cancelled())
                .await
                .is_err()
        {
            return Err(ApiError::Conflict(
                "session loop did not stop in time; try again".into(),
            ));
        }
        self.store.delete_session(id).await?;
        self.errors.lock().await.remove(id);
        if let Some(hub) = self.hubs.lock().await.remove(id) {
            hub.publish(|seq| Envelope::Deleted { seq });
        }
        Ok(())
    }

    pub async fn export(&self, id: &str) -> Result<SessionExport, ApiError> {
        let stored = self.store.load_state(id).await?;
        let calls = self.store.list_llm_calls(id).await?;
        let cfg = settings(&stored.meta.config)?;
        let provider = match &cfg.provider_id {
            Some(pid) => {
                self.registry
                    .config
                    .get_provider(pid)
                    .await
                    .ok()
                    .map(|p| ProviderSummary {
                        id: p.id,
                        name: p.name,
                        kind: p.kind,
                        base_url: p.base_url,
                    })
            }
            None => None,
        };
        let state = &stored.state;
        Ok(SessionExport {
            format: "oort.session.v1",
            exported_at: crate::now_ms(),
            provider,
            system_prompt: state.system_prompt().to_string(),
            frozen_tools: state.tool_snapshot().map(<[_]>::to_vec).unwrap_or_default(),
            messages: state
                .messages()
                .iter()
                .enumerate()
                .map(|(seq, m)| RawMessage {
                    seq,
                    role: m.role,
                    content: m.content.clone(),
                    usage: stored.message_usage.get(seq).cloned().flatten(),
                })
                .collect(),
            compactions: state
                .checkpoints()
                .iter()
                .enumerate()
                .map(|(i, c)| CheckpointView {
                    epoch: i + 1,
                    up_to_seq: c.up_to_index,
                    summary: c.summary_message.clone(),
                })
                .collect(),
            llm_calls: calls,
            phase: state.phase.clone(),
            pending_steers: state.pending_steers.iter().cloned().collect(),
            settings: cfg,
            meta: stored.meta,
        })
    }

    /// Starts the loop again after it exited with an error (e.g. once a rate limit has passed).
    pub async fn retry(&self, id: &str) -> Result<(), ApiError> {
        self.store.load_state(id).await?;
        self.retries.lock().await.remove(id);
        self.errors.lock().await.remove(id);
        self.ensure_active(id).await.map(|_| ())
    }

    /// Called by the retry timer: resumes only if the same failure is still waiting for it.
    async fn auto_retry(&self, id: &str, at: i64) {
        {
            let mut errors = self.errors.lock().await;
            match errors.get(id) {
                Some(e) if e.at == at && e.retry_at.is_some() => {
                    errors.remove(id);
                }
                _ => return,
            }
        }
        *self.retries.lock().await.entry(id.to_string()).or_default() += 1;
        if let Err(e) = self.ensure_active(id).await {
            tracing::warn!(session = %id, error = %e, "automatic retry could not start");
        }
    }

    pub async fn cancel(&self, id: &str) -> Result<bool, ApiError> {
        if let Some(e) = self.errors.lock().await.get_mut(id) {
            e.retry_at = None;
        }
        for t in self.skill_runs.lock().await.remove(id).unwrap_or_default() {
            t.cancel();
        }
        Ok(match self.active.lock().await.get(id) {
            Some(a) => {
                a.cancel.cancel();
                true
            }
            None => false,
        })
    }

    /// Subscribes to a session's events (resuming after `last_event_id` = `{boot}:{seq}` when
    /// possible) and resumes the loop if the session has pending work.
    pub async fn subscribe(
        &self,
        id: &str,
        last_event_id: Option<&str>,
    ) -> Result<EventStream, ApiError> {
        let stored = self.store.load_state(id).await?;
        let after = last_event_id
            .and_then(|v| v.split_once(':'))
            .filter(|(boot, _)| *boot == self.boot)
            .and_then(|(_, seq)| seq.parse::<u64>().ok());
        let sub = self.hub(id).await.subscribe(after);
        // A loop that failed (quota, network, …) waits for an explicit retry or new input instead
        // of being resumed by every reconnect.
        let failed = self.errors.lock().await.contains_key(id);
        if stored.meta.status == SessionStatus::Running && !failed {
            self.ensure_active(id).await?;
        }
        let active = self.active.lock().await.contains_key(id);
        Ok(EventStream {
            hello: Envelope::Hello {
                seq: sub.seq,
                boot: self.boot.clone(),
                resumed: sub.replay.is_some(),
                status: stored.meta.status,
                active,
                live: sub.live,
            },
            replay: sub.replay.unwrap_or_default(),
            rx: sub.rx,
        })
    }

    /// Cancels every loop (their state is already persisted) and closes MCP connections.
    pub async fn shutdown(&self) {
        self.closing.cancel();
        for t in std::mem::take(&mut *self.skill_runs.lock().await)
            .into_values()
            .flatten()
        {
            t.cancel();
        }
        for a in self.active.lock().await.values() {
            a.cancel.cancel();
        }
        self.registry.close().await;
    }

    /// Resolves once [`SessionManager::shutdown`] has started.
    pub fn closing(&self) -> CancellationToken {
        self.closing.clone()
    }

    async fn hub(&self, id: &str) -> Arc<Hub> {
        self.hubs
            .lock()
            .await
            .entry(id.to_string())
            .or_insert_with(|| Arc::new(Hub::new()))
            .clone()
    }

    /// Returns the inbox of the session's loop, loading and spawning it if needed.
    async fn ensure_active(&self, id: &str) -> Result<mpsc::Sender<LoopInput>, ApiError> {
        let mut active = self.active.lock().await;
        if let Some(a) = active.get(id) {
            return Ok(a.inbox.clone());
        }
        let stored = self.store.load_state(id).await?;
        if stored.meta.status == SessionStatus::Halted {
            return Err(ApiError::Conflict("session is halted".into()));
        }
        let cfg = settings(&stored.meta.config)?;
        let hub = self.hub(id).await;
        let cancel = CancellationToken::new();
        let approval = ApprovalHandle::new(stored.meta.approval_policy);
        let effort = stored.meta.reasoning_effort.or(self
            .registry
            .provider_defaults(&cfg)
            .await
            .reasoning_effort);
        let thinking: oort_agent::LiveThinking =
            Arc::new(std::sync::RwLock::new(effort.thinking()));
        let (ev_tx, mut ev_rx) = mpsc::channel(1024);
        let agent = self
            .registry
            .build_agent(
                &cfg,
                &stored.meta.lineage,
                id,
                &stored.state,
                Arc::clone(&self.store),
                approval.clone(),
                ev_tx,
                cancel.clone(),
                Arc::clone(&thinking),
            )
            .await?;
        let (in_tx, in_rx) = mpsc::channel(64);
        let generation = self.generation.fetch_add(1, Ordering::Relaxed);
        let finished = CancellationToken::new();

        let fwd_hub = Arc::clone(&hub);
        let forwarder = tokio::spawn(async move {
            while let Some(event) = ev_rx.recv().await {
                // Widgets are announced as soon as the assistant message declaring them is
                // committed (the history keeps them; this is a convenience for live clients).
                let widgets: Vec<(String, serde_json::Value)> = match &event {
                    AgentEvent::AssistantMessage { message } => message
                        .content
                        .iter()
                        .filter_map(|p| match p {
                            ContentPart::ToolCall(c) if c.name == RENDER_WIDGET => {
                                Some((c.id.clone(), c.arguments.clone()))
                            }
                            _ => None,
                        })
                        .collect(),
                    _ => vec![],
                };
                fwd_hub.publish(|seq| Envelope::Agent { seq, event });
                for (call_id, widget) in widgets {
                    fwd_hub.publish(|seq| Envelope::Widget {
                        seq,
                        call_id,
                        widget,
                    });
                }
            }
        });

        let registry = Arc::clone(&self.active);
        let errors = Arc::clone(&self.errors);
        errors.lock().await.remove(id);
        let retries = Arc::clone(&self.retries);
        let auto = self.registry.auto_retry(&cfg).await;
        let me = self.me.clone();
        let closing = self.closing.clone();
        let session_id = id.to_string();
        let mut state = stored.state;
        let done = finished.clone();
        tokio::spawn(async move {
            let mut inbox = Inbox::new(in_rx);
            let result = agent.run_loop(&mut state, &mut inbox).await;
            drop(agent); // closes the event channel so the forwarder drains and ends
            let _ = forwarder.await;
            match &result {
                Ok(_) => {
                    retries.lock().await.remove(&session_id);
                }
                Err(oort_agent::AgentError::Cancelled) => {}
                Err(e) => {
                    tracing::warn!(session = %session_id, error = %e, "session loop exited with error");
                    let message = e.to_string();
                    let at = crate::now_ms();
                    let attempt = retries.lock().await.get(&session_id).copied().unwrap_or(0);
                    let delay = (auto.enabled && attempt < auto.max_attempts && is_transient(e))
                        .then(|| retry_delay(&message, attempt));
                    errors.lock().await.insert(
                        session_id.clone(),
                        LoopError {
                            message,
                            at,
                            retry_at: delay.map(|d| at + d.as_millis() as i64),
                            attempt,
                            max_attempts: auto.max_attempts,
                        },
                    );
                    if let Some(delay) = delay {
                        let id = session_id.clone();
                        tokio::spawn(async move {
                            tokio::select! {
                                _ = tokio::time::sleep(delay) => {}
                                _ = closing.cancelled() => return,
                            }
                            if let Some(m) = me.upgrade() {
                                m.auto_retry(&id, at).await;
                            }
                        });
                    }
                }
            }
            hub.publish(|seq| Envelope::LoopExited {
                seq,
                error: result.err().map(|e| e.to_string()),
            });
            let mut reg = registry.lock().await;
            if reg
                .get(&session_id)
                .is_some_and(|a| a.generation == generation)
            {
                reg.remove(&session_id);
            }
            done.cancel();
        });

        active.insert(
            id.to_string(),
            ActiveSession {
                inbox: in_tx.clone(),
                cancel,
                approval,
                thinking,
                finished,
                generation,
            },
        );
        Ok(in_tx)
    }
}

/// Rate limits, overloads, server errors and dropped connections: worth retrying unchanged.
/// Extends [`oort_llm::Error::is_retryable`] with stream drops and status-less quota errors.
fn is_transient(e: &oort_agent::AgentError) -> bool {
    let oort_agent::AgentError::Llm(err) = e else {
        return false;
    };
    match err {
        oort_llm::Error::Stream(_) => true,
        oort_llm::Error::Api {
            status: None,
            message,
            ..
        } => {
            let m = message.to_ascii_lowercase();
            ["rate limit", "resource_exhausted", "overloaded", "quota"]
                .iter()
                .any(|k| m.contains(k))
        }
        other => other.is_retryable(),
    }
}

/// The provider's hint (`Please retry in 13.09s`, `"retryDelay": "13s"`) plus a second, or an
/// exponential backoff (5s, 10s, 20s, …, at most 2 minutes).
fn retry_delay(message: &str, attempt: u32) -> Duration {
    let hinted = ["retry in ", "retryDelay\": \"", "retryDelay\":\""]
        .iter()
        .find_map(|k| {
            let rest = &message[message.find(k)? + k.len()..];
            let n: String = rest
                .chars()
                .take_while(|c| c.is_ascii_digit() || *c == '.')
                .collect();
            n.parse::<f64>().ok()
        });
    match hinted {
        Some(secs) if secs.is_finite() && secs >= 0.0 => {
            Duration::from_secs_f64(secs.min(600.0) + 1.0)
        }
        _ => Duration::from_secs(5u64.saturating_mul(1 << attempt.min(5)).min(120)),
    }
}

/// Result of interpreting user input with explicit skills / a leading `/command`.
enum Expanded {
    Compact,
    Text(String),
    /// Handed to a skill subagent; its report is sent to the session when it finishes.
    Deferred,
}

impl SessionManager {
    /// Explicit skills (`invoked_skills`, or a leading `/name args`) are appended to *this*
    /// user message as `<active_skill>` blocks — never to the system prompt or older
    /// messages; the engine replaces repeats within an epoch by a short marker. Skills in
    /// `subagent` mode run in an isolated subagent whose report is sent back later.
    /// `/compact` is a command. Other text is unchanged.
    async fn prepare_user_input(
        &self,
        id: &str,
        text: &str,
        mut invoked: Vec<InvokedSkill>,
    ) -> Result<Expanded, ApiError> {
        let mut rest = text.to_string();
        let trimmed = text.trim_start();
        if let Some(cmd) = trimmed.strip_prefix('/') {
            let (name, args) = cmd
                .split_once(char::is_whitespace)
                .map(|(n, r)| (n, r.trim()))
                .unwrap_or((cmd, ""));
            if name == "compact" {
                return Ok(Expanded::Compact);
            }
            if oort_skills::valid_name(name) {
                invoked.insert(
                    0,
                    InvokedSkill {
                        name: name.to_string(),
                        arguments: Some(args.to_string()).filter(|a| !a.is_empty()),
                    },
                );
                rest = String::new();
            }
        }
        if invoked.is_empty() {
            return Ok(Expanded::Text(text.to_string()));
        }
        let mut blocks = Vec::new();
        let mut delegated = Vec::new();
        for inv in &invoked {
            let skill = self
                .registry
                .skills
                .by_name(&inv.name)
                .await?
                .ok_or_else(|| {
                    ApiError::BadRequest(format!("unknown command or skill /{}", inv.name))
                })?;
            if !skill.invocation.user_invocable {
                return Err(ApiError::BadRequest(format!(
                    "/{} cannot be invoked directly",
                    skill.name
                )));
            }
            match skill.invocation.execution_mode {
                SkillExecutionMode::Inline => {
                    blocks.push(skill.invocation_block(inv.arguments.as_deref()))
                }
                SkillExecutionMode::Subagent => delegated.push((skill, inv.arguments.clone())),
            }
        }
        let rest = rest.trim().to_string();
        if !delegated.is_empty() {
            self.delegate_skills(id, delegated, blocks, rest).await?;
            return Ok(Expanded::Deferred);
        }
        let ask = if rest.is_empty() {
            "Follow the skill above.".to_string()
        } else {
            rest
        };
        blocks.push(ask);
        Ok(Expanded::Text(blocks.join("\n\n")))
    }

    /// Runs `subagent`-mode skills in isolated children (progress is relayed to this session's
    /// stream), then sends their reports — plus any inline skills and the user's text — to the
    /// session as one ordinary user message.
    async fn delegate_skills(
        &self,
        id: &str,
        skills: Vec<(oort_skills::Skill, Option<String>)>,
        inline_blocks: Vec<String>,
        user_text: String,
    ) -> Result<(), ApiError> {
        let stored = self.store.load_state(id).await?;
        let cfg = settings(&stored.meta.config)?;
        let hub = self.hub(id).await;
        let cancel = CancellationToken::new();
        self.skill_runs
            .lock()
            .await
            .entry(id.to_string())
            .or_default()
            .push(cancel.clone());
        let (dummy_tx, _dummy_rx) = mpsc::channel(1);
        let runtime = SubagentRuntime {
            session_id: id.to_string(),
            cfg,
            lineage: stored.meta.lineage.clone(),
            store: self.store.clone(),
            events: dummy_tx,
        };
        let registry = self.registry.clone();
        let me = self.me.clone();
        let session_id = id.to_string();
        tokio::spawn(async move {
            let mut reports = Vec::new();
            let mut runs = Vec::new();
            for (skill, args) in skills {
                let call_id = format!("skill:{}", crate::new_id());
                let task = SubagentTask {
                    task: format!(
                        "{}\n\n{}",
                        skill.invocation_block(args.as_deref()),
                        if user_text.is_empty() {
                            "Follow the skill above."
                        } else {
                            &user_text
                        }
                    ),
                    context: None,
                    profile: skill
                        .invocation
                        .subagent_profile
                        .clone()
                        .unwrap_or_else(|| "explorer".into()),
                    mode: ContextMode::Fresh,
                    tools: None,
                };
                let hub = hub.clone();
                let emit: EventFn =
                    Arc::new(move |event| hub.publish(|seq| Envelope::Agent { seq, event }));
                let (registry, runtime, cancel) =
                    (registry.clone(), runtime.clone(), cancel.clone());
                runs.push((
                    skill.name.clone(),
                    tokio::spawn(async move {
                        run_subagent(&registry, &runtime, &call_id, task, cancel, emit).await
                    }),
                ));
            }
            for (name, run) in runs {
                let report = match run.await {
                    Ok(Ok(o)) => format!(
                        "<subagent_result skill=\"{name}\" session=\"{}\" ending=\"{}\">\n{}\n</subagent_result>",
                        o.sub_session_id, o.ending, o.summary
                    ),
                    Ok(Err(e)) => format!(
                        "<subagent_result skill=\"{name}\" error=\"true\">\n{e}\n</subagent_result>"
                    ),
                    Err(e) => format!(
                        "<subagent_result skill=\"{name}\" error=\"true\">\n{e}\n</subagent_result>"
                    ),
                };
                reports.push(report);
            }
            if cancel.is_cancelled() {
                return;
            }
            let mut parts = inline_blocks;
            parts.extend(reports);
            if !user_text.is_empty() {
                parts.push(user_text);
            }
            if let Some(me) = me.upgrade()
                && let Err(e) = me
                    .input(
                        &session_id,
                        InputRequest::Steer {
                            text: parts.join("\n\n"),
                            invoked_skills: vec![],
                        },
                    )
                    .await
            {
                tracing::warn!(session = %session_id, error = %e, "could not deliver skill subagent report");
            }
        });
        Ok(())
    }
}

/// Canonical, existing directory for the workspace tools.
fn canonical_workspace(root: &str) -> Result<String, ApiError> {
    let p = std::path::Path::new(root.trim())
        .canonicalize()
        .map_err(|e| ApiError::BadRequest(format!("workspace_root {root}: {e}")))?;
    if !p.is_dir() {
        return Err(ApiError::BadRequest(format!(
            "workspace_root {root} is not a directory"
        )));
    }
    Ok(p.to_string_lossy().to_string())
}

fn settings(raw: &serde_json::Value) -> Result<SessionConfig, ApiError> {
    serde_json::from_value(raw.clone())
        .map_err(|e| ApiError::Internal(format!("session config: {e}")))
}
