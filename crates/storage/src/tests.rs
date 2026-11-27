use std::sync::Arc;

use oort_agent::testing::{Behavior, MockModel, MockTool, MockTurn, executor};
use oort_agent::*;
use oort_llm::{ChatRequest, Message, PromptCacheConfig, Role};

use super::*;

const SYSTEM: &str = "You are a careful coding agent.";

fn context() -> DefaultContextManager {
    DefaultContextManager::new(RequestParams {
        model: Some("mock-model".into()),
        prompt_cache: Some(PromptCacheConfig::enabled()),
        ..Default::default()
    })
}

fn bytes<T: serde::Serialize>(v: &T) -> Vec<u8> {
    serde_json::to_vec(v).unwrap()
}

/// `next` must be a strict byte-level extension of `prev` (ignoring moving cache markers).
fn assert_prefix_stable(prev: &ChatRequest, next: &ChatRequest) {
    let mut a = prev.clone();
    let mut b = next.clone();
    a.cache_breakpoints.clear();
    b.cache_breakpoints.clear();
    assert!(b.messages.len() >= a.messages.len());
    b.messages.truncate(a.messages.len());
    assert_eq!(bytes(&a), bytes(&b));
}

struct Harness {
    store: Arc<SqliteSessionStore>,
    tools: Vec<Arc<MockTool>>,
}

impl Harness {
    async fn new(tools: Vec<Arc<MockTool>>) -> Self {
        Self {
            store: Arc::new(SqliteSessionStore::in_memory().await.unwrap()),
            tools,
        }
    }

    /// Builds an agent that persists to `id`, starting from `state` (as stored).
    fn agent(
        &self,
        model: Arc<MockModel>,
        id: &str,
        state: &AgentState,
        extra: Vec<Arc<dyn LoopHook>>,
    ) -> Agent {
        let mut b =
            Agent::builder(model, executor(&self.tools)).context_manager(Arc::new(context()));
        for h in extra {
            b = b.hook(h);
        }
        b.hook(Arc::new(PersistenceHook::new(
            self.store.clone(),
            id.to_string(),
            state,
        )))
        .build()
        .unwrap()
    }

    async fn create(&self, initial: Vec<Message>) -> (SessionId, AgentState) {
        let frozen = FrozenToolset::freeze(executor(&self.tools).as_ref()).unwrap();
        let state = AgentState::with_tools(SYSTEM, initial, &frozen);
        let id = self
            .store
            .create_session(
                NewSession {
                    title: "t".into(),
                    provider: "mock".into(),
                    model: "mock-model".into(),
                    ..Default::default()
                },
                &state,
            )
            .await
            .unwrap();
        (id, state)
    }
}

#[tokio::test]
async fn needs_approval_pause_reload_resume_keeps_prefix() {
    let read = MockTool::new("read", true, Behavior::Ok("r".into()));
    let write = MockTool::new("write", false, Behavior::Ok("w".into()));
    let h = Harness::new(vec![write.clone(), read.clone()]).await;
    let policy: Arc<dyn LoopHook> = Arc::new(ApprovalPolicy::default());
    let (id, mut state) = h.create(vec![Message::user("edit the file")]).await;

    // --- run until the approval pause; every step is committed by PersistenceHook.
    let model1 = MockModel::new(vec![MockTurn::tools(
        &[("c1", "read", "{}"), ("c2", "write", "{}")],
        100,
    )]);
    let agent1 = h.agent(model1, &id, &state, vec![policy.clone()]);
    agent1
        .run_loop(&mut state, &mut Inbox::detached())
        .await
        .unwrap();
    assert!(matches!(state.phase, Phase::AwaitingApproval { .. }));
    let preview_before = agent1.preview_request(&state);
    drop(agent1);

    // --- reload: the state round-trips exactly, including the pending approval.
    let stored = h.store.load_state(&id).await.unwrap();
    assert_eq!(stored.meta.status, SessionStatus::NeedsApproval);
    assert_eq!(stored.state, state);
    assert_eq!(
        stored.message_usage[1].as_ref().map(|u| u.input_tokens),
        Some(100)
    );
    assert_eq!(stored.message_usage[0], None);

    // Frozen prefix is byte-identical: stored JSON == executor catalog, and the next request matches.
    let model2 = MockModel::new(vec![MockTurn::text("edited", 120)]);
    let agent2 = h.agent(model2.clone(), &id, &stored.state, vec![policy]);
    let frozen: String = sqlx::query_scalar("SELECT frozen_tools FROM sessions WHERE id = ?")
        .bind(&id)
        .fetch_one(h.store.pool())
        .await
        .unwrap();
    assert_eq!(frozen, canonical_tools_json(agent2.tools().definitions()));
    assert_eq!(
        bytes(&agent2.preview_request(&stored.state)),
        bytes(&preview_before)
    );

    // --- approve and finish.
    let mut state = stored.state;
    let (tx, mut inbox) = Inbox::channel(4);
    tx.send(LoopInput::Approval {
        call_id: "c2".into(),
        decision: ApprovalDecision::Approve,
    })
    .await
    .unwrap();
    drop(tx);
    agent2.run_loop(&mut state, &mut inbox).await.unwrap();
    assert_eq!((read.runs(), write.runs()), (1, 1));
    assert_prefix_stable(&preview_before, &model2.requests()[0]);

    let done = h.store.load_state(&id).await.unwrap();
    assert_eq!(done.meta.status, SessionStatus::Idle);
    assert_eq!(done.state, state);
    let roles: Vec<Role> = done.state.messages().iter().map(|m| m.role).collect();
    assert_eq!(
        roles,
        [
            Role::User,
            Role::Assistant,
            Role::Tool,
            Role::Tool,
            Role::Assistant
        ]
    );
    assert_eq!(
        done.message_usage[4].as_ref().map(|u| u.input_tokens),
        Some(120)
    );
}

#[tokio::test]
async fn compaction_is_persisted_and_resumes_in_same_epoch() {
    let read = MockTool::new("read", true, Behavior::Ok("r".into()));
    let h = Harness::new(vec![read]).await;
    let (id, mut state) = h.create(vec![Message::user("long task")]).await;
    let mut policy = CompactionPolicy::new(1000);
    policy.keep_recent_messages = 2;

    let model = MockModel::new(vec![
        MockTurn::tools(&[("c1", "read", "{}")], 100),
        MockTurn::tools(&[("c2", "read", "{}")], 900),
        MockTurn::text("paused", 200),
    ]);
    let build = |model: Arc<MockModel>, state: &AgentState| {
        Agent::builder(model, executor(&h.tools))
            .context_manager(Arc::new(context().with_compaction(policy.clone())))
            .compactor(Arc::new(RuleBasedCompactor {
                max_chars_per_message: 40,
            }))
            .hook(Arc::new(PersistenceHook::new(
                h.store.clone(),
                id.clone(),
                state,
            )))
            .build()
            .unwrap()
    };
    let agent = build(model.clone(), &state);
    agent
        .run_loop(&mut state, &mut Inbox::detached())
        .await
        .unwrap();
    assert_eq!(state.checkpoints().len(), 1);
    let preview = agent.preview_request(&state);

    let stored = h.store.load_state(&id).await.unwrap();
    assert_eq!(stored.state, state, "raw history and checkpoint survive");
    assert_eq!(stored.state.messages().len(), 6);
    assert_eq!(stored.state.checkpoints()[0].up_to_index, 3);

    // After reload, the next turn extends the *new epoch's* prefix byte-for-byte.
    let model2 = MockModel::new(vec![MockTurn::text("resumed", 210)]);
    let agent2 = build(model2.clone(), &stored.state);
    assert_eq!(
        bytes(&agent2.preview_request(&stored.state)),
        bytes(&preview)
    );
    let mut state = stored.state;
    let (tx, mut inbox) = Inbox::channel(4);
    tx.send(LoopInput::Steer(Message::user("continue")))
        .await
        .unwrap();
    drop(tx);
    agent2.run_loop(&mut state, &mut inbox).await.unwrap();
    let req = &model2.requests()[0];
    assert_eq!(req.messages[0], state.checkpoints()[0].summary_message);
    assert_prefix_stable(&preview, req);
    assert_eq!(h.store.load_state(&id).await.unwrap().state, state);
}

#[tokio::test]
async fn schema_enforces_append_only_and_frozen_prefix() {
    let h = Harness::new(vec![]).await;
    let (id, state) = h
        .create(vec![Message::user("a"), Message::assistant("b")])
        .await;
    let pool = h.store.pool();

    for sql in [
        "UPDATE messages SET content_json = '[]' WHERE session_id = ?",
        "DELETE FROM messages WHERE session_id = ?",
        "UPDATE sessions SET system_prompt = 'changed' WHERE id = ?",
        "UPDATE sessions SET frozen_tools = '[{\"name\":\"x\"}]' WHERE id = ?",
        "DELETE FROM sessions WHERE id = ?",
    ] {
        let err = sqlx::query(sql)
            .bind(&id)
            .execute(pool)
            .await
            .expect_err(sql);
        assert!(err.to_string().contains("oort:"), "{sql}: {err}");
    }

    // Gaps and duplicates are conflicts, not silent overwrites.
    let msg = Message::user("c");
    assert!(matches!(
        h.store.append_message(&id, 5, &msg, None).await,
        Err(StoreError::Conflict(_))
    ));
    assert!(matches!(
        h.store.append_message(&id, 1, &msg, None).await,
        Err(StoreError::Conflict(_))
    ));
    h.store.append_message(&id, 2, &msg, None).await.unwrap();

    // Checkpoints must advance within the history.
    let cp = |up_to| CompactionCheckpoint {
        up_to_index: up_to,
        summary_message: Message::user("s"),
    };
    assert!(matches!(
        h.store.save_compaction(&id, 1, &cp(9)).await,
        Err(StoreError::Conflict(_))
    ));
    assert!(matches!(
        h.store.save_compaction(&id, 2, &cp(1)).await,
        Err(StoreError::Conflict(_))
    ));
    h.store.save_compaction(&id, 1, &cp(1)).await.unwrap();
    assert!(matches!(
        h.store.save_compaction(&id, 2, &cp(1)).await,
        Err(StoreError::Conflict(_))
    ));

    // Unaffected rows are still intact.
    let loaded = h.store.load_state(&id).await.unwrap();
    assert_eq!(loaded.state.messages()[..2], state.messages()[..]);
    assert_eq!(loaded.state.system_prompt(), SYSTEM);
}

#[tokio::test]
async fn list_and_not_found_and_catalog_mismatch() {
    let read = MockTool::new("read", true, Behavior::Ok("r".into()));
    let h = Harness::new(vec![read]).await;
    let (a, _) = h.create(vec![Message::user("first")]).await;
    let (b, sb) = h.create(vec![Message::user("second")]).await;
    h.store
        .update_session_status(&a, &SessionSnapshot::from_state(&sb))
        .await
        .unwrap();

    let list = h.store.list_sessions().await.unwrap();
    assert_eq!(list.len(), 2);
    assert!(
        list.iter()
            .any(|m| m.id == b && m.message_count == 1 && m.status == SessionStatus::Running)
    );
    assert!(list[0].updated_at >= list[1].updated_at);
    assert!(matches!(
        h.store.load_state("nope").await,
        Err(StoreError::NotFound(_))
    ));

    // Resuming with a different catalog is refused before any request is sent.
    let stored = h.store.load_state(&b).await.unwrap();
    let other = Harness {
        store: h.store.clone(),
        tools: vec![MockTool::new("other", true, Behavior::Ok("".into()))],
    };
    let agent = other.agent(MockModel::new(vec![]), &b, &stored.state, vec![]);
    let mut state = stored.state;
    assert!(matches!(
        agent.step(&mut state, &mut Inbox::detached()).await,
        Err(AgentError::ToolCatalogMismatch(_))
    ));
}

#[tokio::test]
async fn file_database_uses_wal_and_foreign_keys() {
    let dir = std::env::temp_dir().join(format!("oort-storage-{}", uuid::Uuid::now_v7()));
    std::fs::create_dir_all(&dir).unwrap();
    let url = format!("sqlite://{}", dir.join("t.db").display());
    let store = SqliteSessionStore::connect(&url).await.unwrap();
    let mode: String = sqlx::query_scalar("PRAGMA journal_mode")
        .fetch_one(store.pool())
        .await
        .unwrap();
    let fk: i64 = sqlx::query_scalar("PRAGMA foreign_keys")
        .fetch_one(store.pool())
        .await
        .unwrap();
    assert_eq!(mode, "wal");
    assert_eq!(fk, 1);
    // Orphan rows are rejected by the foreign key.
    let err = store
        .append_message("missing", 0, &Message::user("x"), None)
        .await
        .unwrap_err();
    assert!(matches!(err, StoreError::Conflict(_)), "{err}");
    drop(store);
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn canonical_tool_json_is_key_sorted() {
    let def = oort_llm::ToolDefinition::new(
        "t",
        "d",
        serde_json::from_str(r#"{"type":"object","properties":{"z":{},"a":{}}}"#).unwrap(),
    );
    let json = canonical_tools_json(&[def]);
    assert!(json.find("\"a\"").unwrap() < json.find("\"z\"").unwrap());
}
