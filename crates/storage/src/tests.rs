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
        "UPDATE sessions SET config_json = '{}' WHERE id = ?",
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

// ----- workbench additions ---------------------------------------------------------------

#[tokio::test]
async fn llm_calls_are_committed_with_their_step() {
    let h = Harness::new(vec![]).await;
    let (id, mut state) = h.create(vec![Message::user("hi")]).await;
    let model = MockModel::new(vec![MockTurn::text("hello", 42)]);
    let agent = h.agent(model, &id, &state, vec![]);
    agent
        .run_loop(&mut state, &mut Inbox::detached())
        .await
        .unwrap();
    let calls = h.store.list_llm_calls(&id).await.unwrap();
    assert_eq!(calls.len(), 1);
    assert_eq!((calls[0].turn, calls[0].seq), (1, 1));
    assert_eq!(calls[0].request.messages, 1);
    assert_eq!(calls[0].usage.as_ref().unwrap().input_tokens, 42);
    // Append-only like the rest of the history.
    let err = sqlx::query("UPDATE llm_calls SET latency_ms = 0")
        .execute(h.store.pool())
        .await
        .unwrap_err();
    assert!(err.to_string().contains("append-only"), "{err}");
}

#[tokio::test]
async fn title_and_policy_are_mutable_prefix_is_not() {
    let h = Harness::new(vec![]).await;
    let (id, _) = h.create(vec![Message::user("hi")]).await;
    h.store.set_title(&id, "renamed").await.unwrap();
    h.store
        .set_approval_policy(&id, ApprovalPolicyKind::AlwaysAsk)
        .await
        .unwrap();
    let meta = h.store.load_state(&id).await.unwrap().meta;
    assert_eq!(meta.title, "renamed");
    assert_eq!(meta.approval_policy, ApprovalPolicyKind::AlwaysAsk);
    assert!(matches!(
        h.store.set_title("nope", "x").await,
        Err(StoreError::NotFound(_))
    ));
    let err = sqlx::query("UPDATE sessions SET system_prompt = 'x'")
        .execute(h.store.pool())
        .await
        .unwrap_err();
    assert!(err.to_string().contains("immutable"));
}

#[tokio::test]
async fn delete_session_removes_everything_but_raw_deletes_stay_blocked() {
    let h = Harness::new(vec![]).await;
    let (id, mut state) = h.create(vec![Message::user("hi")]).await;
    let (other, _) = h.create(vec![Message::user("keep")]).await;
    let agent = h.agent(
        MockModel::new(vec![MockTurn::text("yo", 5)]),
        &id,
        &state,
        vec![],
    );
    agent
        .run_loop(&mut state, &mut Inbox::detached())
        .await
        .unwrap();

    for sql in [
        "DELETE FROM messages",
        "DELETE FROM llm_calls",
        "DELETE FROM sessions",
    ] {
        let err = sqlx::query(sql).execute(h.store.pool()).await.unwrap_err();
        assert!(err.to_string().contains("oort:"), "{sql}: {err}");
    }

    h.store.delete_session(&id).await.unwrap();
    assert!(matches!(
        h.store.load_state(&id).await,
        Err(StoreError::NotFound(_))
    ));
    for table in ["messages", "llm_calls", "compactions"] {
        let n: i64 = sqlx::query_scalar(&format!(
            "SELECT COUNT(*) FROM {table} WHERE session_id = ?"
        ))
        .bind(&id)
        .fetch_one(h.store.pool())
        .await
        .unwrap();
        assert_eq!(n, 0, "{table}");
    }
    assert_eq!(
        h.store
            .load_state(&other)
            .await
            .unwrap()
            .state
            .messages()
            .len(),
        1
    );
    assert!(matches!(
        h.store.delete_session(&id).await,
        Err(StoreError::NotFound(_))
    ));
}

fn provider(id: &str, name: &str) -> ProviderRecord {
    ProviderRecord {
        id: id.into(),
        name: name.into(),
        kind: ProviderKind::Gemini,
        base_url: None,
        api_key: Some("env:GEMINI_API_KEY".into()),
        default_model: "gemini-2.5-flash".into(),
        extra_headers: [("x-org".to_string(), "acme".to_string())].into(),
        is_enabled: true,
        created_at: 1,
        updated_at: 1,
    }
}

#[tokio::test]
async fn config_crud_and_reference_checks() {
    let s = SqliteSessionStore::in_memory().await.unwrap();
    // Seeded offline provider.
    let seeded = s.get_provider("mock").await.unwrap();
    assert_eq!(seeded.kind, ProviderKind::Mock);

    s.create_provider(&provider("p1", "Gemini")).await.unwrap();
    assert!(matches!(
        s.create_provider(&provider("p2", "Gemini")).await,
        Err(StoreError::Conflict(_))
    ));
    let mut p = s.get_provider("p1").await.unwrap();
    assert_eq!(p, provider("p1", "Gemini"));
    p.default_model = "gemini-2.5-pro".into();
    p.updated_at = 2;
    s.update_provider(&p).await.unwrap();
    assert_eq!(
        s.get_provider("p1").await.unwrap().default_model,
        "gemini-2.5-pro"
    );
    assert_eq!(s.list_providers().await.unwrap().len(), 2);

    let mcp = McpServerRecord {
        id: "m1".into(),
        name: "fs".into(),
        transport: McpTransportKind::Stdio,
        url: None,
        command: Some("mcp-fs".into()),
        args: vec!["--root".into(), ".".into()],
        env: [("TOKEN".to_string(), "env:FS_TOKEN".to_string())].into(),
        headers: Default::default(),
        auth: McpAuth::None,
        is_enabled: true,
        created_at: 1,
        updated_at: 1,
    };
    s.create_mcp_server(&mcp).await.unwrap();
    assert_eq!(s.get_mcp_server("m1").await.unwrap(), mcp);

    let preset = PresetRecord {
        id: "r1".into(),
        name: "coder".into(),
        description: "d".into(),
        provider_id: "p1".into(),
        model: String::new(),
        system_prompt: "be terse".into(),
        mcp_server_ids: vec!["m1".into()],
        builtin_tools: false,
        approval_policy: ApprovalPolicyKind::AlwaysAsk,
        skill_ids: vec!["file:review".into()],
        builtin_groups: vec!["memory".into()],
        workspace_root: None,
        created_at: 1,
        updated_at: 1,
    };
    s.create_preset(&preset).await.unwrap();
    assert_eq!(s.list_presets().await.unwrap(), vec![preset.clone()]);

    // Referenced records cannot be deleted; the error names the preset.
    for r in [
        s.delete_provider("p1").await,
        s.delete_mcp_server("m1").await,
    ] {
        match r {
            Err(StoreError::Conflict(m)) => assert!(m.contains("coder"), "{m}"),
            other => panic!("{other:?}"),
        }
    }
    // A preset pointing at an unknown provider is rejected by the foreign key.
    let mut bad = preset.clone();
    bad.id = "r2".into();
    bad.name = "bad".into();
    bad.provider_id = "missing".into();
    assert!(matches!(
        s.create_preset(&bad).await,
        Err(StoreError::Conflict(_))
    ));

    s.delete_preset("r1").await.unwrap();
    s.delete_mcp_server("m1").await.unwrap();
    s.delete_provider("p1").await.unwrap();
    assert!(matches!(
        s.get_provider("p1").await,
        Err(StoreError::NotFound(_))
    ));
    assert!(matches!(
        s.update_preset(&preset).await,
        Err(StoreError::NotFound(_))
    ));
}

#[tokio::test]
async fn oauth_tokens_upsert_keep_long_scopes_and_cascade() {
    let s = SqliteSessionStore::in_memory().await.unwrap();
    let server = McpServerRecord {
        id: "m1".into(),
        name: "remote".into(),
        transport: McpTransportKind::StreamableHttp,
        url: Some("http://x/mcp".into()),
        command: None,
        args: vec![],
        env: Default::default(),
        headers: Default::default(),
        auth: McpAuth::OAuth {
            config: Default::default(),
        },
        is_enabled: true,
        created_at: 1,
        updated_at: 1,
    };
    s.create_mcp_server(&server).await.unwrap();
    assert!(s.get_oauth_token("m1").await.unwrap().is_none());

    let long_scope = (0..700)
        .map(|i| format!("scope:{i}"))
        .collect::<Vec<_>>()
        .join(" ");
    assert!(long_scope.len() > 4096);
    let mut t = OAuthTokenRecord {
        server_id: "m1".into(),
        issuer: "http://as".into(),
        resource: "http://x/mcp".into(),
        token_endpoint: "http://as/token".into(),
        registration_endpoint: Some("http://as/register".into()),
        client_id: "c1".into(),
        client_secret: None,
        client_source: OAuthClientSource::Dcr,
        access_token: Some("at".into()),
        refresh_token: Some("rt".into()),
        expires_at: Some(123),
        scopes: long_scope.clone(),
        status: OAuthStatus::Connected,
        updated_at: 1,
    };
    s.put_oauth_token(&t).await.unwrap();
    assert_eq!(s.get_oauth_token("m1").await.unwrap().unwrap(), t);
    t.access_token = Some("at2".into());
    t.status = OAuthStatus::AuthRequired;
    s.put_oauth_token(&t).await.unwrap();
    let got = s.get_oauth_token("m1").await.unwrap().unwrap();
    assert_eq!(got, t);
    assert_eq!(got.scopes, long_scope, "never truncated");
    // Refreshing tokens never touches the server record (it versions pooled connections).
    assert_eq!(s.get_mcp_server("m1").await.unwrap().updated_at, 1);

    s.delete_mcp_server("m1").await.unwrap();
    assert!(s.get_oauth_token("m1").await.unwrap().is_none(), "cascades");
}

#[tokio::test]
async fn skills_crud() {
    let s = SqliteSessionStore::in_memory().await.unwrap();
    let mut k = SkillRecord {
        id: "k1".into(),
        name: "code-review".into(),
        description: "Review a diff".into(),
        body: "# Steps\n1. read".into(),
        allowed_tools: vec!["read_file".into()],
        tags: vec!["dev".into()],
        invocation: Default::default(),
        created_at: 1,
        updated_at: 1,
    };
    s.create_skill(&k).await.unwrap();
    let mut dup = k.clone();
    dup.id = "k2".into();
    assert!(matches!(
        s.create_skill(&dup).await,
        Err(StoreError::Conflict(_))
    ));
    k.body = "changed".into();
    s.update_skill(&k).await.unwrap();
    assert_eq!(s.get_skill("k1").await.unwrap(), k);
    assert_eq!(s.list_skills().await.unwrap().len(), 1);
    s.delete_skill("k1").await.unwrap();
    assert!(matches!(
        s.get_skill("k1").await,
        Err(StoreError::NotFound(_))
    ));
}

// ----- agent foundation ----------------------------------------------------------------------

fn memory(id: &str, title: &str, content: &str, tags: &[&str], scope: &str) -> MemoryRecord {
    MemoryRecord {
        id: id.into(),
        title: title.into(),
        content: content.into(),
        tags: tags.iter().map(|t| t.to_string()).collect(),
        scope: scope.into(),
        created_at: 1,
        updated_at: 1,
    }
}

#[tokio::test]
async fn memories_full_text_search_in_japanese_chinese_and_english() {
    let s = SqliteSessionStore::in_memory().await.unwrap();
    for m in [
        memory(
            "m1",
            "コーディング規約",
            "このプロジェクトではタブではなくスペース4つでインデントする",
            &["style"],
            "global",
        ),
        memory(
            "m2",
            "部署配置",
            "生产环境部署使用蓝绿发布策略，回滚时间不超过五分钟",
            &["ops"],
            "global",
        ),
        memory(
            "m3",
            "Deploy checklist",
            "Always run the database migrations before restarting the API servers",
            &["ops"],
            "proj-a",
        ),
        memory(
            "m4",
            "好み",
            "ユーザーは返答を簡潔な箇条書きで受け取るのを好む",
            &["prefs"],
            "global",
        ),
    ] {
        s.create_memory(&m).await.unwrap();
    }
    let ids = |hits: Vec<MemoryHit>| hits.into_iter().map(|h| h.memory.id).collect::<Vec<_>>();
    let q = |text: &str| MemoryQuery {
        text: text.into(),
        ..Default::default()
    };

    assert_eq!(
        ids(s.search_memories(&q("インデント")).await.unwrap()),
        ["m1"]
    );
    assert_eq!(
        ids(s.search_memories(&q("蓝绿发布")).await.unwrap()),
        ["m2"]
    );
    assert_eq!(
        ids(s.search_memories(&q("database migrations")).await.unwrap()),
        ["m3"]
    );
    assert_eq!(
        ids(s.search_memories(&q("箇条書き")).await.unwrap()),
        ["m4"]
    );
    // Short (< 3 chars) queries fall back to LIKE.
    assert_eq!(ids(s.search_memories(&q("規約")).await.unwrap()), ["m1"]);
    assert_eq!(ids(s.search_memories(&q("回滚")).await.unwrap()), ["m2"]);
    // Tag and scope filters; FTS scores are positive.
    let ops = s
        .search_memories(&MemoryQuery {
            text: String::new(),
            tags: vec!["ops".into()],
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(ops.len(), 2);
    let scoped = s
        .search_memories(&MemoryQuery {
            text: "migrations".into(),
            scope: Some("proj-a".into()),
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(scoped.len(), 1);
    assert!(scoped[0].score > 0.0);
    assert!(
        s.search_memories(&MemoryQuery {
            text: "migrations".into(),
            scope: Some("global".into()),
            ..Default::default()
        })
        .await
        .unwrap()
        .is_empty()
    );
    // Punctuation in queries is literal, not FTS syntax.
    assert!(s.search_memories(&q("\"AND OR*")).await.is_ok());

    // Updates are re-indexed; deletes leave the index.
    let mut m = s.get_memory("m3").await.unwrap();
    m.content = "Rotate the TLS certificates every quarter".into();
    s.update_memory(&m).await.unwrap();
    assert!(
        s.search_memories(&q("migrations"))
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        ids(s.search_memories(&q("certificates")).await.unwrap()),
        ["m3"]
    );
    s.delete_memory("m3").await.unwrap();
    assert!(
        s.search_memories(&q("certificates"))
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn subagent_sessions_form_a_tree_hidden_from_the_list() {
    let h = Harness::new(vec![]).await;
    let (parent, _) = h.create(vec![Message::user("p")]).await;
    let frozen = FrozenToolset::freeze(executor(&[]).as_ref()).unwrap();
    let child_state = AgentState::with_tools("child", vec![Message::user("c")], &frozen);
    let mk = |pid: &str, depth: u32| NewSession {
        provider: "mock".into(),
        model: "m".into(),
        lineage: Lineage {
            parent_session_id: Some(pid.into()),
            parent_tool_call_id: Some("call_1".into()),
            depth,
            workspace_root: Some("/tmp/ws".into()),
            builtin_groups: vec!["utilities".into()],
        },
        ..Default::default()
    };
    let child = h
        .store
        .create_session(mk(&parent, 1), &child_state)
        .await
        .unwrap();
    let grandchild = h
        .store
        .create_session(mk(&child, 2), &child_state)
        .await
        .unwrap();

    let top: Vec<String> = h
        .store
        .list_sessions()
        .await
        .unwrap()
        .into_iter()
        .map(|m| m.id)
        .collect();
    assert_eq!(top, std::slice::from_ref(&parent), "children are not top-level sessions");
    let kids = h.store.list_children(&parent).await.unwrap();
    assert_eq!(kids.len(), 1);
    assert_eq!(kids[0].lineage.depth, 1);
    assert_eq!(kids[0].lineage.builtin_groups, ["utilities"]);
    assert_eq!(kids[0].lineage.workspace_root.as_deref(), Some("/tmp/ws"));

    let err = sqlx::query("UPDATE sessions SET workspace_root = '/' WHERE id = ?")
        .bind(&child)
        .execute(h.store.pool())
        .await
        .unwrap_err();
    assert!(err.to_string().contains("immutable"), "{err}");

    // Deleting the parent removes the whole tree.
    h.store.delete_session(&parent).await.unwrap();
    for id in [&parent, &child, &grandchild] {
        assert!(matches!(
            h.store.load_state(id).await,
            Err(StoreError::NotFound(_))
        ));
    }
}

#[tokio::test]
async fn loaded_skills_and_skill_invocation_round_trip() {
    let h = Harness::new(vec![]).await;
    let (id, mut state) = h.create(vec![Message::user("x")]).await;
    state.loaded_skills.insert("review".into());
    h.store
        .update_session_status(&id, &SessionSnapshot::from_state(&state))
        .await
        .unwrap();
    let loaded = h.store.load_state(&id).await.unwrap().state;
    assert!(loaded.loaded_skills.contains("review"));

    let k = SkillRecord {
        id: "k".into(),
        name: "deep-dive".into(),
        description: "d".into(),
        body: "b".into(),
        allowed_tools: vec![],
        tags: vec![],
        invocation: SkillInvocation {
            user_invocable: false,
            argument_hint: Some("<topic>".into()),
            execution_mode: SkillExecutionMode::Subagent,
            subagent_profile: Some("researcher".into()),
        },
        created_at: 1,
        updated_at: 1,
    };
    h.store.create_skill(&k).await.unwrap();
    assert_eq!(h.store.get_skill("k").await.unwrap(), k);
}
