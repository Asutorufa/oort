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
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use oort_agent::{
    AgentEvent, AgentState, FrozenToolset, Inbox, LoopInput, NewSession, SessionStatus,
    SessionStore,
};
use oort_llm::{ContentPart, Message};
use tokio::sync::{Mutex, broadcast, mpsc};
use tokio_util::sync::CancellationToken;

use crate::api::{
    ApiError, CheckpointView, CreateSessionRequest, Envelope, InputRequest, PatchSessionRequest,
    ProviderSummary, RawMessage, SessionDetail, SessionExport, SessionSummary,
};
use crate::approval::ApprovalHandle;
use crate::config::{SessionConfig, SkillBinding};
use crate::hub::Hub;
use crate::registry::{Registry, ToolSelection};
use crate::session_tools::RENDER_WIDGET;

/// Handles to a running loop.
struct ActiveSession {
    inbox: mpsc::Sender<LoopInput>,
    cancel: CancellationToken,
    approval: Arc<ApprovalHandle>,
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
}

impl SessionManager {
    pub fn new(store: Arc<dyn SessionStore>, registry: Arc<Registry>) -> Arc<Self> {
        Arc::new(Self {
            store,
            registry,
            active: Arc::default(),
            hubs: Mutex::default(),
            generation: AtomicU64::new(0),
            boot: crate::new_id(),
            closing: CancellationToken::new(),
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
            max_context_tokens: req.max_context_tokens.unwrap_or(200_000),
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
        };
        let id = self.store.create_session(meta, &state).await?;
        self.ensure_active(&id).await?;
        Ok(id)
    }

    pub async fn list(&self) -> Result<Vec<SessionSummary>, ApiError> {
        let active = self.active.lock().await;
        Ok(self
            .store
            .list_sessions()
            .await?
            .into_iter()
            .map(|meta| SessionSummary {
                active: active.contains_key(&meta.id),
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
        Ok(SessionDetail::build(
            stored.meta,
            active,
            cfg,
            &stored.state,
            &stored.message_usage,
            &calls,
        ))
    }

    pub async fn input(&self, id: &str, input: InputRequest) -> Result<(), ApiError> {
        let input = match input {
            InputRequest::Steer { text } => match self.expand_command(&text).await? {
                Expanded::Compact => LoopInput::Compact,
                Expanded::Text(t) => LoopInput::Steer(Message::user(t)),
            },
            InputRequest::Interrupt { text } => match self.expand_command(&text).await? {
                Expanded::Compact => LoopInput::Compact,
                Expanded::Text(t) => LoopInput::Interrupt(Message::user(t)),
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
        let meta = self.store.load_state(id).await?.meta;
        self.hub(id).await.publish(|seq| Envelope::SessionUpdated {
            seq,
            title: meta.title,
            approval_policy: meta.approval_policy,
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

    pub async fn cancel(&self, id: &str) -> Result<bool, ApiError> {
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
        if stored.meta.status == SessionStatus::Running {
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
        let (ev_tx, mut ev_rx) = mpsc::channel(1024);
        let agent = self
            .registry
            .build_agent(
                &cfg,
                id,
                &stored.state,
                Arc::clone(&self.store),
                approval.clone(),
                ev_tx,
                cancel.clone(),
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
        let session_id = id.to_string();
        let mut state = stored.state;
        let done = finished.clone();
        tokio::spawn(async move {
            let mut inbox = Inbox::new(in_rx);
            let result = agent.run_loop(&mut state, &mut inbox).await;
            drop(agent); // closes the event channel so the forwarder drains and ends
            let _ = forwarder.await;
            if let Err(e) = &result {
                tracing::warn!(session = %session_id, error = %e, "session loop exited with error");
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
                finished,
                generation,
            },
        );
        Ok(in_tx)
    }
}

/// Result of interpreting a leading `/command` in user input.
enum Expanded {
    Compact,
    Text(String),
}

impl SessionManager {
    /// `/compact` → compaction; `/<skill> rest` → the skill's body followed by `rest`, appended at
    /// the tail like any user message (the frozen prefix is untouched). Other text is unchanged.
    async fn expand_command(&self, text: &str) -> Result<Expanded, ApiError> {
        let trimmed = text.trim_start();
        let Some(cmd) = trimmed.strip_prefix('/') else {
            return Ok(Expanded::Text(text.to_string()));
        };
        let (name, rest) = cmd
            .split_once(char::is_whitespace)
            .map(|(n, r)| (n, r.trim()))
            .unwrap_or((cmd, ""));
        if name == "compact" {
            return Ok(Expanded::Compact);
        }
        if !oort_skills::valid_name(name) {
            return Ok(Expanded::Text(text.to_string()));
        }
        let skill = self
            .registry
            .skills
            .by_name(name)
            .await?
            .ok_or_else(|| ApiError::BadRequest(format!("unknown command or skill /{name}")))?;
        let ask = if rest.is_empty() {
            "Follow this skill.".to_string()
        } else {
            rest.to_string()
        };
        Ok(Expanded::Text(format!("{}\n\n{ask}", skill.render_body())))
    }
}

fn settings(raw: &serde_json::Value) -> Result<SessionConfig, ApiError> {
    serde_json::from_value(raw.clone())
        .map_err(|e| ApiError::Internal(format!("session config: {e}")))
}
