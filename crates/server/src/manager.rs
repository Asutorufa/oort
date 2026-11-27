//! Runs one background `run_loop` per active session and fans its events out to SSE clients.
//!
//! ```text
//!  POST /input ──► inbox (mpsc) ──► run_loop ──► AgentEvent (mpsc) ──► forwarder ──► Hub (broadcast) ──► SSE
//!                                     │ after_step
//!                                     ▼
//!                               SQLite (PersistenceHook) ◄── GET /sessions/:id (snapshot)
//! ```
//!
//! Sessions are activated lazily (on create, input, or when a `running` session is watched),
//! loaded from the store, and removed from the active map when their loop exits.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use oort_agent::{AgentState, Inbox, LoopInput, NewSession, SessionStatus, SessionStore};
use oort_llm::Message;
use tokio::sync::{Mutex, broadcast, mpsc};
use tokio_util::sync::CancellationToken;

use crate::api::{
    ApiError, CreateSessionRequest, Envelope, InputRequest, SessionDetail, SessionSummary,
};
use crate::config::SessionConfig;
use crate::factory::AgentFactory;

/// Per-session event hub. Outlives individual loop runs so SSE clients stay connected
/// across restarts/resumes.
pub struct Hub {
    tx: broadcast::Sender<Envelope>,
    seq: AtomicU64,
}

impl Hub {
    fn new() -> Self {
        Self {
            tx: broadcast::channel(1024).0,
            seq: AtomicU64::new(0),
        }
    }

    fn next_seq(&self) -> u64 {
        self.seq.fetch_add(1, Ordering::Relaxed) + 1
    }

    pub fn subscribe(&self) -> broadcast::Receiver<Envelope> {
        self.tx.subscribe()
    }
}

/// Handles to a running loop.
struct ActiveSession {
    inbox: mpsc::Sender<LoopInput>,
    cancel: CancellationToken,
    generation: u64,
}

pub struct SessionManager {
    store: Arc<dyn SessionStore>,
    factory: Arc<AgentFactory>,
    active: Arc<Mutex<BTreeMap<String, ActiveSession>>>,
    hubs: Mutex<BTreeMap<String, Arc<Hub>>>,
    generation: AtomicU64,
    /// Cancelled by [`SessionManager::shutdown`]; ends open SSE streams so the HTTP server
    /// can finish its graceful shutdown.
    closing: CancellationToken,
}

impl SessionManager {
    pub fn new(store: Arc<dyn SessionStore>, factory: Arc<AgentFactory>) -> Arc<Self> {
        Arc::new(Self {
            store,
            factory,
            active: Arc::default(),
            hubs: Mutex::default(),
            generation: AtomicU64::new(0),
            closing: CancellationToken::new(),
        })
    }

    pub async fn create(&self, req: CreateSessionRequest) -> Result<String, ApiError> {
        let cfg = SessionConfig {
            provider: req.provider,
            model: req.model.trim().to_string(),
            max_context_tokens: req.max_context_tokens.unwrap_or(200_000),
            max_tokens: req.max_tokens,
            approval: req.approval,
        };
        if cfg.model.is_empty() {
            return Err(ApiError::BadRequest("model must not be empty".into()));
        }
        self.factory.model(&cfg)?; // fail fast on missing API keys
        let initial: Vec<Message> = req
            .message
            .filter(|m| !m.trim().is_empty())
            .map(Message::user)
            .into_iter()
            .collect();
        let state = AgentState::with_tools(req.system_prompt, initial, self.factory.frozen());
        let meta = NewSession {
            id: None,
            title: req.title.unwrap_or_default(),
            provider: cfg.provider.as_str().into(),
            model: cfg.model.clone(),
            config: serde_json::to_value(&cfg).map_err(|e| ApiError::Internal(e.to_string()))?,
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
        let cfg = settings(&stored.meta.config)?;
        let active = self.active.lock().await.contains_key(id);
        Ok(SessionDetail::build(
            stored.meta,
            active,
            cfg,
            &stored.state,
            &stored.message_usage,
        ))
    }

    pub async fn input(&self, id: &str, input: InputRequest) -> Result<(), ApiError> {
        let input = match input {
            InputRequest::Steer { text } => LoopInput::Steer(Message::user(text)),
            InputRequest::Interrupt { text } => LoopInput::Interrupt(Message::user(text)),
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

    pub async fn cancel(&self, id: &str) -> Result<bool, ApiError> {
        Ok(match self.active.lock().await.get(id) {
            Some(a) => {
                a.cancel.cancel();
                true
            }
            None => false,
        })
    }

    /// Subscribes to a session's events; resumes the loop if the session has pending work.
    pub async fn subscribe(&self, id: &str) -> Result<broadcast::Receiver<Envelope>, ApiError> {
        let stored = self.store.load_state(id).await?;
        let rx = self.hub(id).await.subscribe();
        if stored.meta.status == SessionStatus::Running {
            self.ensure_active(id).await?;
        }
        Ok(rx)
    }

    /// Cancels every loop (their state is already persisted) and closes MCP connections.
    pub async fn shutdown(&self) {
        self.closing.cancel();
        for a in self.active.lock().await.values() {
            a.cancel.cancel();
        }
        self.factory.close().await;
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
        let (ev_tx, mut ev_rx) = mpsc::channel(1024);
        let agent = self.factory.build_agent(
            &cfg,
            id,
            &stored.state,
            Arc::clone(&self.store),
            ev_tx,
            cancel.clone(),
        )?;
        let (in_tx, in_rx) = mpsc::channel(64);
        let generation = self.generation.fetch_add(1, Ordering::Relaxed);

        let fwd_hub = Arc::clone(&hub);
        let forwarder = tokio::spawn(async move {
            while let Some(event) = ev_rx.recv().await {
                let _ = fwd_hub.tx.send(Envelope::Agent {
                    seq: fwd_hub.next_seq(),
                    event,
                });
            }
        });

        let registry = Arc::clone(&self.active);
        let session_id = id.to_string();
        let mut state = stored.state;
        tokio::spawn(async move {
            let mut inbox = Inbox::new(in_rx);
            let result = agent.run_loop(&mut state, &mut inbox).await;
            drop(agent); // closes the event channel so the forwarder drains and ends
            let _ = forwarder.await;
            if let Err(e) = &result {
                tracing::warn!(session = %session_id, error = %e, "session loop exited with error");
            }
            let _ = hub.tx.send(Envelope::LoopExited {
                seq: hub.next_seq(),
                error: result.err().map(|e| e.to_string()),
            });
            let mut reg = registry.lock().await;
            if reg
                .get(&session_id)
                .is_some_and(|a| a.generation == generation)
            {
                reg.remove(&session_id);
            }
        });

        active.insert(
            id.to_string(),
            ActiveSession {
                inbox: in_tx.clone(),
                cancel,
                generation,
            },
        );
        Ok(in_tx)
    }
}

fn settings(raw: &serde_json::Value) -> Result<SessionConfig, ApiError> {
    serde_json::from_value(raw.clone())
        .map_err(|e| ApiError::Internal(format!("session config: {e}")))
}
