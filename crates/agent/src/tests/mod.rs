//! Scenario tests for the agent loop using scripted mocks.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use oort_llm::{ChatRequest, ContentPart, Message, PromptCacheConfig, Role, ToolResult};
use tokio::sync::{Barrier, Notify, mpsc};
use tokio::task::JoinHandle;

use crate::testing::{Behavior, FailingModel, Gate, MockModel, MockTool, MockTurn, executor};
use crate::*;

const SYSTEM: &str = "You are a careful coding agent.";

// ----- helpers ---------------------------------------------------------------------------

fn cached_context() -> DefaultContextManager {
    DefaultContextManager::new(RequestParams {
        model: Some("mock-model".into()),
        max_tokens: Some(1024),
        prompt_cache: Some(PromptCacheConfig::enabled().with_key("session-1")),
        ..Default::default()
    })
}

fn build(model: Arc<dyn ModelClient>, tools: &[Arc<MockTool>]) -> AgentBuilder {
    Agent::builder(model, executor(tools)).context_manager(Arc::new(cached_context()))
}

fn spawn_loop(
    agent: Arc<Agent>,
    mut state: AgentState,
    mut inbox: Inbox,
) -> JoinHandle<(AgentState, Result<(), AgentError>)> {
    tokio::spawn(async move {
        let r = agent.run_loop(&mut state, &mut inbox).await;
        (state, r)
    })
}

async fn join(
    h: JoinHandle<(AgentState, Result<(), AgentError>)>,
) -> (AgentState, Result<(), AgentError>) {
    tokio::time::timeout(Duration::from_secs(5), h)
        .await
        .expect("loop did not finish in time")
        .unwrap()
}

fn drain(rx: &mut mpsc::Receiver<AgentEvent>) -> Vec<AgentEvent> {
    let mut out = Vec::new();
    while let Ok(e) = rx.try_recv() {
        out.push(e);
    }
    out
}

async fn wait_for(rx: &mut mpsc::Receiver<AgentEvent>, want: fn(&AgentEvent) -> bool) {
    let fut = async {
        while let Some(e) = rx.recv().await {
            if want(&e) {
                return;
            }
        }
        panic!("event stream ended");
    };
    tokio::time::timeout(Duration::from_secs(5), fut)
        .await
        .expect("event not observed");
}

fn results(msg: &Message) -> Vec<&ToolResult> {
    msg.content
        .iter()
        .filter_map(|p| match p {
            ContentPart::ToolResult(r) => Some(r),
            _ => None,
        })
        .collect()
}

fn call_ids(msg: &Message) -> Vec<&str> {
    msg.content
        .iter()
        .filter_map(|p| match p {
            ContentPart::ToolCall(c) => Some(c.id.as_str()),
            _ => None,
        })
        .collect()
}

/// Every ToolCall is answered by ToolResults before any other message, and vice versa.
fn assert_protocol_valid(messages: &[Message]) {
    let mut open: Vec<String> = Vec::new();
    for (i, m) in messages.iter().enumerate() {
        if m.role == Role::Tool {
            for r in results(m) {
                let pos = open
                    .iter()
                    .position(|id| *id == r.tool_call_id)
                    .unwrap_or_else(|| panic!("orphan tool result {} at {i}", r.tool_call_id));
                open.remove(pos);
            }
        } else {
            assert!(
                open.is_empty(),
                "message {i} interleaves unresolved calls {open:?}"
            );
            open.extend(call_ids(m).into_iter().map(String::from));
        }
    }
    assert!(open.is_empty(), "unresolved calls at end: {open:?}");
}

/// `next` must be a strict byte-level extension of `prev`.
fn assert_prefix_stable(prev: &ChatRequest, next: &ChatRequest) {
    let bytes = |v: &dyn erased::Ser| v.to_bytes();
    assert_eq!(bytes(&prev.system), bytes(&next.system), "system changed");
    assert_eq!(bytes(&prev.tools), bytes(&next.tools), "tools changed");
    assert!(next.messages.len() > prev.messages.len());
    for (i, (a, b)) in prev.messages.iter().zip(&next.messages).enumerate() {
        assert_eq!(bytes(a), bytes(b), "message {i} was rewritten");
    }
    // Whole request (minus moving cache markers) is a prefix extension.
    let mut a = prev.clone();
    let mut b = next.clone();
    a.cache_breakpoints.clear();
    b.cache_breakpoints.clear();
    b.messages.truncate(a.messages.len());
    assert_eq!(bytes(&a), bytes(&b), "request parameters changed");
}

mod erased {
    pub trait Ser {
        fn to_bytes(&self) -> Vec<u8>;
    }
    impl<T: serde::Serialize> Ser for T {
        fn to_bytes(&self) -> Vec<u8> {
            serde_json::to_vec(self).unwrap()
        }
    }
}

// ----- 1. multi-turn tool calling --------------------------------------------------------

#[tokio::test]
async fn multi_turn_tool_calling_loop() {
    let read = MockTool::new("read_file", true, Behavior::Ok("contents".into()));
    let write = MockTool::new("write_file", false, Behavior::Ok("written".into()));
    let model = MockModel::new(vec![
        MockTurn::tools(&[("c1", "read_file", r#"{"path":"a.rs"}"#)], 100),
        MockTurn::tools(
            &[("c2", "write_file", r#"{"path":"a.rs","content":"x"}"#)],
            150,
        ),
        MockTurn::text("All done.", 200),
    ]);
    let agent = build(model.clone(), &[read.clone(), write.clone()])
        .build()
        .unwrap();
    let mut state = agent.new_state(SYSTEM, vec![Message::user("Fix a.rs")]);

    agent
        .run_loop(&mut state, &mut Inbox::detached())
        .await
        .unwrap();

    let roles: Vec<Role> = state.messages().iter().map(|m| m.role).collect();
    use Role::*;
    assert_eq!(roles, [User, Assistant, Tool, Assistant, Tool, Assistant]);
    assert_eq!(state.phase, Phase::Idle);
    assert_eq!(state.turn, 3);
    assert_eq!(state.usage.llm_calls, 3);
    assert_eq!((read.runs(), write.runs()), (1, 1));
    assert!(
        results(&state.messages()[2])[0]
            .content
            .starts_with("contents")
    );
    assert_eq!(
        results(&state.messages()[4])[0].name.as_deref(),
        Some("write_file")
    );
    assert_eq!(state.messages()[5].text(), "All done.");
    assert_protocol_valid(state.messages());
    assert_eq!(model.requests().len(), 3);
}

#[tokio::test]
async fn read_only_tools_run_concurrently() {
    let barrier = Arc::new(Barrier::new(2));
    let a = MockTool::new("grep_a", true, Behavior::Barrier(barrier.clone()));
    let b = MockTool::new("grep_b", true, Behavior::Barrier(barrier));
    let model = MockModel::new(vec![
        MockTurn::tools(&[("c1", "grep_a", "{}"), ("c2", "grep_b", "{}")], 10),
        MockTurn::text("ok", 10),
    ]);
    let agent = build(model, &[a, b]).build().unwrap();
    let mut state = agent.new_state(SYSTEM, vec![Message::user("search")]);
    agent
        .run_loop(&mut state, &mut Inbox::detached())
        .await
        .unwrap();

    // Sequential execution would deadlock on the barrier and time out into an error.
    for (m, id) in state.messages()[2..4].iter().zip(["c1", "c2"]) {
        let r = results(m)[0];
        assert_eq!(r.tool_call_id, id, "results keep call order");
        assert!(!r.is_error, "{}", r.content);
    }
}

// ----- 2. prefix stability ---------------------------------------------------------------

#[tokio::test]
async fn request_prefix_is_byte_stable_across_turns() {
    // Registered out of order: the frozen catalog must be sorted by name.
    let tools = [
        MockTool::new("zeta_write", false, Behavior::Ok("w".into())),
        MockTool::new("alpha_read", true, Behavior::Ok("r".into())),
        MockTool::new("mid_search", true, Behavior::Ok("s".into())),
    ];
    let model = MockModel::new(vec![
        MockTurn::tools(
            &[("c1", "alpha_read", r#"{"path":"x","content":"y"}"#)],
            100,
        ),
        MockTurn::tools(
            &[("c2", "mid_search", "{}"), ("c3", "alpha_read", "{}")],
            120,
        ),
        MockTurn::tools(
            &[("c4", "zeta_write", r#"{"content":"b","path":"a"}"#)],
            140,
        ),
        MockTurn::text("done", 160),
    ]);
    let agent = build(model.clone(), &tools).build().unwrap();
    let mut state = agent.new_state(SYSTEM, vec![Message::user("go")]);
    agent
        .run_loop(&mut state, &mut Inbox::detached())
        .await
        .unwrap();

    let reqs = model.requests();
    assert_eq!(reqs.len(), 4);
    let names: Vec<&str> = reqs[0].tools.iter().map(|t| t.name.as_str()).collect();
    assert_eq!(names, ["alpha_read", "mid_search", "zeta_write"]);
    for pair in reqs.windows(2) {
        assert_prefix_stable(&pair[0], &pair[1]);
    }

    // Abstract breakpoints: static prefix + previous tail + current tail.
    let last = reqs.last().unwrap();
    let n = last.messages.len();
    let prev_tail = last
        .messages
        .iter()
        .rposition(|m| m.role == Role::Assistant)
        .unwrap()
        - 1;
    assert_eq!(
        last.cache_breakpoints,
        [
            oort_llm::CacheBreakpoint::ToolsAndSystem,
            oort_llm::CacheBreakpoint::AfterMessage { index: prev_tail },
            oort_llm::CacheBreakpoint::AfterMessage { index: n - 1 },
        ]
    );

    // Rebuilding the agent yields byte-identical tool schemas (deterministic serialization).
    let again = build(MockModel::new(vec![]), &tools).build().unwrap();
    assert_eq!(
        serde_json::to_vec(again.tools().definitions()).unwrap(),
        serde_json::to_vec(&reqs[0].tools).unwrap()
    );
}

/// Dynamic info (e.g. a clock) is appended to the tail and never edits the prefix.
#[tokio::test]
async fn hook_appended_context_goes_to_tail() {
    struct Clock;
    #[async_trait]
    impl LoopHook for Clock {
        async fn before_llm(&self, ctx: &mut BeforeLlm<'_>) -> HookAction {
            ctx.append_tail(Message::system(format!(
                "<env>turn={}</env>",
                ctx.state.turn
            )));
            HookAction::Continue
        }
    }
    let read = MockTool::new("read", true, Behavior::Ok("r".into()));
    let model = MockModel::new(vec![
        MockTurn::tools(&[("c1", "read", "{}")], 10),
        MockTurn::text("done", 10),
    ]);
    let agent = build(model.clone(), &[read])
        .hook(Arc::new(Clock))
        .build()
        .unwrap();
    let mut state = agent.new_state(SYSTEM, vec![Message::user("go")]);
    agent
        .run_loop(&mut state, &mut Inbox::detached())
        .await
        .unwrap();

    let reqs = model.requests();
    assert_prefix_stable(&reqs[0], &reqs[1]);
    let tail = reqs[1].messages.last().unwrap();
    assert_eq!(
        tail.role,
        Role::User,
        "system notices are demoted to user to keep the system prompt fixed"
    );
    assert_eq!(tail.text(), "<env>turn=1</env>");
    assert_eq!(reqs[1].system.as_deref(), Some(SYSTEM));
}

// ----- 3. steer --------------------------------------------------------------------------

#[tokio::test]
async fn steer_during_stream_is_applied_next_turn() {
    let reached = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let model = MockModel::new(vec![
        MockTurn::text("first answer", 10).with_gate(Gate::Pause {
            after: 1,
            reached: reached.clone(),
            release: release.clone(),
        }),
        MockTurn::text("answer with steer", 10),
    ]);
    let (ev_tx, mut ev_rx) = mpsc::channel(1024);
    let agent = Arc::new(build(model.clone(), &[]).events(ev_tx).build().unwrap());
    let state = agent.new_state(SYSTEM, vec![Message::user("question")]);
    let (tx, inbox) = Inbox::channel(8);
    let handle = spawn_loop(agent, state, inbox);

    reached.notified().await;
    tx.send(LoopInput::Steer(Message::user("also consider X")))
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(20)).await;
    release.notify_one();
    drop(tx);
    let (state, r) = join(handle).await;
    r.unwrap();

    let texts: Vec<String> = state.messages().iter().map(Message::text).collect();
    // The stream was NOT cut: the model's full answer is kept, then the steer, then a new turn
    // (even though the model tried to finish without tool calls).
    assert_eq!(
        texts,
        [
            "question",
            "first answer",
            "also consider X",
            "answer with steer"
        ]
    );
    let reqs = model.requests();
    assert_eq!(reqs[1].messages.last().unwrap().text(), "also consider X");
    assert_prefix_stable(&reqs[0], &reqs[1]);
    let events = drain(&mut ev_rx);
    assert!(events.contains(&AgentEvent::SteerApplied { count: 1 }));
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, AgentEvent::Interrupted { .. }))
    );
}

#[tokio::test]
async fn steer_during_tools_waits_for_results() {
    let started = Arc::new(Notify::new());
    let slow = MockTool::new("slow", false, Behavior::Hang(started.clone()));
    let model = MockModel::new(vec![
        MockTurn::tools(&[("c1", "slow", "{}")], 10),
        MockTurn::text("done", 10),
    ]);
    let agent = Arc::new(build(model.clone(), &[slow]).build().unwrap());
    let cancel = agent.session_cancel().clone();
    let state = agent.new_state(SYSTEM, vec![Message::user("go")]);
    let (tx, inbox) = Inbox::channel(8);
    let handle = spawn_loop(agent, state, inbox);

    started.notified().await;
    tx.send(LoopInput::Steer(Message::user("fyi")))
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(20)).await;
    // The steer must not cancel the tool; it is still running.
    assert!(!handle.is_finished());
    cancel.cancel();
    let (state, r) = join(handle).await;
    assert!(matches!(r, Err(AgentError::Cancelled)));
    // Even on session cancel the history stays protocol-valid; the steer remains queued.
    assert_protocol_valid(state.messages());
    assert_eq!(state.pending_steers.len(), 1);
}

// ----- 4. interrupt ----------------------------------------------------------------------

#[tokio::test]
async fn interrupt_during_stream_discards_partial_output() {
    let reached = Arc::new(Notify::new());
    let model = MockModel::new(vec![
        MockTurn::text("partial output that must vanish", 10).with_gate(Gate::Hang {
            after: 1,
            reached: reached.clone(),
        }),
        MockTurn::text("ok, switching to Y", 10),
    ]);
    let (ev_tx, mut ev_rx) = mpsc::channel(1024);
    let agent = Arc::new(build(model.clone(), &[]).events(ev_tx).build().unwrap());
    let state = agent.new_state(SYSTEM, vec![Message::user("do X")]);
    let (tx, inbox) = Inbox::channel(8);
    let handle = spawn_loop(agent, state, inbox);

    reached.notified().await;
    tx.send(LoopInput::Interrupt(Message::user("stop, do Y")))
        .await
        .unwrap();
    drop(tx);
    let (state, r) = join(handle).await;
    r.unwrap();

    let texts: Vec<String> = state.messages().iter().map(Message::text).collect();
    assert_eq!(texts, ["do X", "stop, do Y", "ok, switching to Y"]);
    let reqs = model.requests();
    assert_prefix_stable(&reqs[0], &reqs[1]);
    assert_eq!(reqs[1].messages.len(), 2);
    let events = drain(&mut ev_rx);
    assert!(events.contains(&AgentEvent::Interrupted {
        during: InterruptPoint::LlmStream
    }));
    // Deltas were streamed to the UI, but never committed to history.
    assert!(
        events
            .iter()
            .any(|e| matches!(e, AgentEvent::TextDelta { .. }))
    );
}

#[tokio::test]
async fn interrupt_during_multiple_tools_synthesizes_results() {
    let started = Arc::new(Notify::new());
    let t1 = MockTool::new("t1_read", true, Behavior::Ok("one".into()));
    let t2 = MockTool::new("t2_build", false, Behavior::Hang(started.clone()));
    let t3 = MockTool::new("t3_deploy", false, Behavior::Ok("three".into()));
    let model = MockModel::new(vec![
        MockTurn::tools(
            &[
                ("c1", "t1_read", "{}"),
                ("c2", "t2_build", "{}"),
                ("c3", "t3_deploy", "{}"),
            ],
            10,
        ),
        MockTurn::text("acknowledged", 10),
    ]);
    let agent = Arc::new(
        build(model.clone(), &[t1.clone(), t2.clone(), t3.clone()])
            .build()
            .unwrap(),
    );
    let state = agent.new_state(SYSTEM, vec![Message::user("ship it")]);
    let (tx, inbox) = Inbox::channel(8);
    let handle = spawn_loop(agent, state, inbox);

    started.notified().await;
    tx.send(LoopInput::Interrupt(Message::user("wait, don't deploy")))
        .await
        .unwrap();
    drop(tx);
    let (state, r) = join(handle).await;
    r.unwrap();

    let msgs = state.messages();
    assert_eq!(call_ids(&msgs[1]), ["c1", "c2", "c3"]);
    let rs: Vec<&ToolResult> = msgs[2..5].iter().map(|m| results(m)[0]).collect();
    assert_eq!(
        rs.iter()
            .map(|r| r.tool_call_id.as_str())
            .collect::<Vec<_>>(),
        ["c1", "c2", "c3"]
    );
    assert!(!rs[0].is_error, "completed result is kept");
    for r in &rs[1..] {
        assert!(r.is_error);
        assert_eq!(r.content, INTERRUPTED_TOOL_RESULT);
        assert!(r.name.is_some(), "Gemini needs the function name");
    }
    // The user message comes only after every tool_call_id is answered.
    assert_eq!(msgs[5].role, Role::User);
    assert_eq!(msgs[5].text(), "wait, don't deploy");
    assert_eq!(msgs[6].text(), "acknowledged");
    assert_protocol_valid(msgs);
    assert_protocol_valid(&model.requests()[1].messages);
    assert_eq!(
        (t1.runs(), t2.runs(), t3.runs()),
        (1, 1, 0),
        "t3 never started"
    );
}

// ----- 5. approval / resume --------------------------------------------------------------

#[tokio::test]
async fn needs_approval_pauses_and_resumes_from_serialized_state() {
    let read = MockTool::new("read", true, Behavior::Ok("r".into()));
    let write = MockTool::new("write", false, Behavior::Ok("w".into()));
    let policy = Arc::new(ApprovalPolicy::default());

    // --- process 1: run until approval is needed, then persist.
    let model1 = MockModel::new(vec![MockTurn::tools(
        &[("c1", "read", "{}"), ("c2", "write", "{}")],
        10,
    )]);
    let agent1 = build(model1, &[read.clone(), write.clone()])
        .hook(policy.clone())
        .build()
        .unwrap();
    let mut state = agent1.new_state(SYSTEM, vec![Message::user("edit")]);
    let mut inbox = Inbox::detached();
    assert_eq!(
        agent1.step(&mut state, &mut inbox).await.unwrap(),
        StepOutcome::Continue
    );
    let outcome = agent1.step(&mut state, &mut inbox).await.unwrap();
    let StepOutcome::NeedsApproval(pending) = outcome else {
        panic!("{outcome:?}")
    };
    assert_eq!(
        pending.iter().map(|c| c.id.as_str()).collect::<Vec<_>>(),
        ["c2"]
    );
    assert!(matches!(state.phase, Phase::AwaitingApproval { .. }));
    assert_eq!(
        (read.runs(), write.runs()),
        (0, 0),
        "nothing runs while paused"
    );
    let saved = serde_json::to_string(&state).unwrap();
    drop((agent1, state));

    // --- process 2: restore and approve.
    let model2 = MockModel::new(vec![MockTurn::text("edited", 10)]);
    let agent2 = Arc::new(
        build(model2.clone(), &[read.clone(), write.clone()])
            .hook(policy)
            .build()
            .unwrap(),
    );
    let restored: AgentState = serde_json::from_str(&saved).unwrap();
    let (tx, inbox) = Inbox::channel(8);
    let handle = spawn_loop(agent2, restored, inbox);
    tx.send(LoopInput::Approval {
        call_id: "c2".into(),
        decision: ApprovalDecision::Approve,
    })
    .await
    .unwrap();
    drop(tx);
    let (state, r) = join(handle).await;
    r.unwrap();

    assert_eq!((read.runs(), write.runs()), (1, 1));
    assert_eq!(state.messages().last().unwrap().text(), "edited");
    assert_eq!(state.phase, Phase::Idle);
    assert_protocol_valid(state.messages());
    assert_eq!(model2.requests()[0].messages.len(), 4);
}

#[tokio::test]
async fn denied_approval_becomes_error_result_and_catalog_is_checked() {
    let write = MockTool::new("write", false, Behavior::Ok("w".into()));
    let model = MockModel::new(vec![
        MockTurn::tools(&[("c1", "write", "{}")], 10),
        MockTurn::text("fine", 10),
    ]);
    let agent = build(model, std::slice::from_ref(&write))
        .hook(Arc::new(ApprovalPolicy::default()))
        .build()
        .unwrap();
    let mut state = agent.new_state(SYSTEM, vec![Message::user("edit")]);
    let (tx, mut inbox) = Inbox::channel(8);
    agent.step(&mut state, &mut inbox).await.unwrap();
    assert!(matches!(
        agent.step(&mut state, &mut inbox).await.unwrap(),
        StepOutcome::NeedsApproval(_)
    ));
    tx.send(LoopInput::Approval {
        call_id: "c1".into(),
        decision: ApprovalDecision::Deny {
            reason: "too risky".into(),
        },
    })
    .await
    .unwrap();
    drop(tx);
    agent.run_loop(&mut state, &mut inbox).await.unwrap();
    assert_eq!(write.runs(), 0);
    let r = results(&state.messages()[2])[0];
    assert!(r.is_error && r.content.contains("too risky"));

    // Resuming with a different tool catalog is refused (it would break the cached prefix).
    let other = build(
        MockModel::new(vec![]),
        &[MockTool::new("other", true, Behavior::Ok("".into()))],
    )
    .build()
    .unwrap();
    assert!(matches!(
        other.step(&mut state, &mut Inbox::detached()).await,
        Err(AgentError::ToolCatalogMismatch(_))
    ));
}

// ----- 6. compaction ---------------------------------------------------------------------

#[tokio::test]
async fn epoch_compaction_on_threshold() {
    let read = MockTool::new("read", true, Behavior::Ok("r".into()));
    let model = MockModel::new(vec![
        MockTurn::tools(&[("c1", "read", "{}")], 100),
        MockTurn::tools(&[("c2", "read", "{}")], 300),
        MockTurn::tools(&[("c3", "read", "{}")], 900), // crosses 80% of 1000
        MockTurn::tools(&[("c4", "read", "{}")], 200),
        MockTurn::tools(&[("c5", "read", "{}")], 250),
        MockTurn::text("done", 260),
    ]);
    let mut policy = CompactionPolicy::new(1000);
    policy.keep_recent_messages = 2;
    let (ev_tx, mut ev_rx) = mpsc::channel(1024);
    let agent = build(model.clone(), &[read])
        .context_manager(Arc::new(cached_context().with_compaction(policy)))
        .compactor(Arc::new(RuleBasedCompactor {
            max_chars_per_message: 40,
        }))
        .events(ev_tx)
        .build()
        .unwrap();
    let mut state = agent.new_state(SYSTEM, vec![Message::user("long task")]);
    agent
        .run_loop(&mut state, &mut Inbox::detached())
        .await
        .unwrap();

    // Exactly one epoch, cut at a turn boundary (before assistant c3, all earlier calls resolved).
    assert_eq!(state.checkpoints().len(), 1);
    let cp = &state.checkpoints()[0];
    assert_eq!(cp.up_to_index, 5);
    assert_eq!(call_ids(&state.messages()[5]), ["c3"]);
    assert!(compact::is_turn_boundary(state.messages(), cp.up_to_index));
    let compacted = drain(&mut ev_rx)
        .into_iter()
        .filter(|e| matches!(e, AgentEvent::Compacted { .. }))
        .count();
    assert_eq!(compacted, 1);

    // Raw history is untouched (append-only): the pre-compaction request is still its prefix.
    let reqs = model.requests();
    assert_eq!(reqs.len(), 6);
    assert_eq!(state.messages().len(), 1 + 5 * 2 + 1);
    assert_eq!(
        &state.messages()[..reqs[2].messages.len()],
        reqs[2].messages.as_slice()
    );
    assert_eq!(state.messages()[0].text(), "long task");

    // Old epoch: stable prefix.
    assert_prefix_stable(&reqs[0], &reqs[1]);
    assert_prefix_stable(&reqs[1], &reqs[2]);
    // New epoch: [summary, messages[5..]], no orphaned tool pairs, stable afterwards.
    assert_eq!(reqs[3].messages[0], cp.summary_message);
    assert!(cp.summary_message.text().contains("long task"));
    assert_eq!(&reqs[3].messages[1..], &state.messages()[5..7]);
    for r in &reqs {
        assert_protocol_valid(&r.messages);
    }
    assert_prefix_stable(&reqs[3], &reqs[4]);
    assert_prefix_stable(&reqs[4], &reqs[5]);
    // System + tools survive the epoch change, so that part of the cache still hits.
    for r in &reqs[1..] {
        assert_eq!(r.system, reqs[0].system);
        assert_eq!(
            serde_json::to_vec(&r.tools).unwrap(),
            serde_json::to_vec(&reqs[0].tools).unwrap()
        );
    }
}

#[tokio::test]
async fn manual_compaction_via_loop_input() {
    let model = MockModel::new(vec![
        MockTurn::text("first reply", 10),
        MockTurn::text("second reply", 10),
    ]);
    let (ev_tx, mut ev_rx) = mpsc::channel(1024);
    let agent = Arc::new(
        build(model.clone(), &[])
            .compactor(Arc::new(RuleBasedCompactor {
                max_chars_per_message: 100,
            }))
            .events(ev_tx)
            .build()
            .unwrap(),
    );
    let state = agent.new_state(SYSTEM, vec![Message::user("hello")]);
    let (tx, inbox) = Inbox::channel(8);
    let handle = spawn_loop(agent, state, inbox);

    // Compaction needs history: wait for the first turn to complete.
    wait_for(&mut ev_rx, |e| matches!(e, AgentEvent::Completed)).await;
    tx.send(LoopInput::Compact).await.unwrap();
    wait_for(&mut ev_rx, |e| matches!(e, AgentEvent::Compacted { .. })).await;
    tx.send(LoopInput::Steer(Message::user("continue")))
        .await
        .unwrap();
    drop(tx);
    let (state, r) = join(handle).await;
    r.unwrap();

    assert_eq!(state.checkpoints().len(), 1);
    let cp = &state.checkpoints()[0];
    assert_eq!(cp.up_to_index, 1);
    let reqs = model.requests();
    assert_eq!(reqs[1].messages[0], cp.summary_message);
    assert!(cp.summary_message.text().contains("hello"));
    assert_eq!(
        reqs[1].messages[1..]
            .iter()
            .map(Message::text)
            .collect::<Vec<_>>(),
        ["first reply", "continue"]
    );
    assert_eq!(state.messages().len(), 4, "raw history keeps everything");
}

// ----- 7. error classification & hooks ---------------------------------------------------

#[tokio::test]
async fn tool_failures_are_fed_back_not_fatal() {
    let bad = MockTool::new("bad", false, Behavior::Fail("file not found: x".into()));
    let good = MockTool::new("good", false, Behavior::Ok("g".into()));
    let model = MockModel::new(vec![
        MockTurn::tools(
            &[
                ("c1", "bad", "{}"),
                ("c2", "good", "{not json"),
                ("c3", "missing", "{}"),
            ],
            10,
        ),
        MockTurn::text("recovered", 10),
    ]);
    let agent = build(model, &[bad, good.clone()]).build().unwrap();
    let mut state = agent.new_state(SYSTEM, vec![Message::user("go")]);
    agent
        .run_loop(&mut state, &mut Inbox::detached())
        .await
        .unwrap();

    let rs: Vec<&ToolResult> = state.messages()[2..5]
        .iter()
        .map(|m| results(m)[0])
        .collect();
    assert!(rs.iter().all(|r| r.is_error));
    assert!(rs[0].content.contains("file not found"));
    assert!(rs[1].content.contains("Invalid JSON"));
    assert!(rs[2].content.contains("unknown tool"));
    assert_eq!(good.runs(), 0, "invalid arguments are never executed");
    assert_eq!(state.messages().last().unwrap().text(), "recovered");
    assert_protocol_valid(state.messages());
}

#[tokio::test]
async fn infrastructure_errors_propagate() {
    let agent = Agent::builder(Arc::new(FailingModel), executor(&[]))
        .build()
        .unwrap();
    let mut state = agent.new_state(SYSTEM, vec![Message::user("hi")]);
    let err = agent
        .run_loop(&mut state, &mut Inbox::detached())
        .await
        .unwrap_err();
    assert!(matches!(err, AgentError::Llm(ref e) if e.status_code() == Some(401)));
    assert_eq!(state.phase, Phase::Ready, "state stays resumable");
    assert_eq!(state.messages().len(), 1);
}

#[tokio::test]
async fn max_turns_and_budget_hooks_halt() {
    let read = MockTool::new("read", true, Behavior::Ok("r".into()));
    let script = || {
        MockModel::new(vec![
            MockTurn::tools(&[("c1", "read", "{}")], 500),
            MockTurn::tools(&[("c2", "read", "{}")], 500),
        ])
    };
    let agent = build(script(), std::slice::from_ref(&read))
        .hook(Arc::new(MaxTurns(1)))
        .build()
        .unwrap();
    let mut state = agent.new_state(SYSTEM, vec![Message::user("go")]);
    agent
        .run_loop(&mut state, &mut Inbox::detached())
        .await
        .unwrap();
    assert!(matches!(&state.phase, Phase::Halted { reason } if reason.contains("max turns")));
    assert_eq!(state.turn, 1);
    assert_protocol_valid(state.messages());

    let agent = build(script(), &[read])
        .hook(Arc::new(TokenBudget {
            max_total_tokens: 400,
        }))
        .build()
        .unwrap();
    let mut state = agent.new_state(SYSTEM, vec![Message::user("go")]);
    agent
        .run_loop(&mut state, &mut Inbox::detached())
        .await
        .unwrap();
    assert!(matches!(&state.phase, Phase::Halted { reason } if reason.contains("budget")));
    // Halting after a tool-call response still answers every call.
    assert_protocol_valid(state.messages());
}

#[tokio::test]
async fn pinned_catalog_survives_live_changes() {
    // Session started with {grep, write}; the live server now has {grep, new_tool}.
    let grep = MockTool::new("grep", true, Behavior::Ok("hit".into()));
    let write = MockTool::new("write", false, Behavior::Ok("w".into()));
    let pinned = FrozenToolset::freeze(executor(&[grep.clone(), write]).as_ref()).unwrap();
    let new_tool = MockTool::new("new_tool", true, Behavior::Ok("n".into()));
    let live = executor(&[grep.clone(), new_tool]);
    let ex = Arc::new(PinnedToolExecutor::new(pinned.definitions(), live));
    assert_eq!(ex.missing(), ["write"]);

    let model = MockModel::new(vec![
        MockTurn::tools(&[("c1", "grep", "{}"), ("c2", "write", "{}")], 10),
        MockTurn::text("ok", 10),
    ]);
    let agent = Agent::builder(model.clone(), ex)
        .context_manager(Arc::new(cached_context()))
        .build()
        .unwrap();
    let mut state = AgentState::with_tools(SYSTEM, vec![Message::user("go")], &pinned);
    agent
        .run_loop(&mut state, &mut Inbox::detached())
        .await
        .unwrap();

    // The model saw exactly the pinned catalog; the vanished tool answered with an error.
    let req = &model.requests.lock().unwrap()[0];
    let names: Vec<&str> = req.tools.iter().map(|t| t.name.as_str()).collect();
    assert_eq!(names, ["grep", "write"]);
    assert!(!results(&state.messages()[2])[0].is_error);
    let gone = results(&state.messages()[3])[0];
    assert!(
        gone.is_error && gone.content.contains("no longer available"),
        "{}",
        gone.content
    );
    assert_eq!(state.phase, Phase::Idle);
}

#[tokio::test]
async fn llm_calls_report_timing_and_request_summary() {
    let model = MockModel::new(vec![MockTurn::text("one", 10), MockTurn::text("two", 20)]);
    let (ev_tx, mut ev_rx) = mpsc::channel(64);
    let agent = build(model, &[]).events(ev_tx).build().unwrap();
    let mut state = agent.new_state(SYSTEM, vec![Message::user("a")]);
    agent
        .step(&mut state, &mut Inbox::detached())
        .await
        .unwrap();
    state.push_message(Message::user("b"));
    state.phase = Phase::Ready;
    agent
        .step(&mut state, &mut Inbox::detached())
        .await
        .unwrap();
    drop(agent);

    let mut stats = Vec::new();
    while let Some(ev) = ev_rx.recv().await {
        if let AgentEvent::LlmCall { stats: s } = ev {
            stats.push(s);
        }
    }
    assert_eq!(stats.len(), 2);
    assert_eq!((stats[0].turn, stats[0].seq), (1, 1));
    assert_eq!((stats[1].turn, stats[1].seq), (2, 3));
    assert!(stats[0].ttft_ms.is_some());
    assert_eq!(stats[1].request.messages, 3);
    // Same system prompt and tools → same prefix hashes; history grew.
    assert_eq!(stats[0].request.hash.system, stats[1].request.hash.system);
    assert_eq!(stats[0].request.hash.tools, stats[1].request.hash.tools);
    assert_ne!(
        stats[0].request.hash.messages,
        stats[1].request.hash.messages
    );
}

// ----- user input & explicit skills ----------------------------------------------------------

/// Asks the user for every call to `ask`.
struct AskHook;

#[async_trait]
impl LoopHook for AskHook {
    async fn before_tool(
        &self,
        _s: &AgentState,
        call: &oort_llm::ToolCall,
        _ro: bool,
    ) -> ToolDecision {
        if call.name == "ask" {
            ToolDecision::NeedUserInput {
                reason: "question for the user".into(),
            }
        } else {
            ToolDecision::Approve
        }
    }
}

#[tokio::test]
async fn user_input_pauses_persists_and_the_answer_becomes_the_result() {
    let ask = MockTool::new("ask", true, Behavior::Ok("never runs".into()));
    let model = MockModel::new(vec![
        MockTurn::tools(&[("c1", "ask", r#"{"question":"which?"}"#)], 10),
        MockTurn::text("thanks", 10),
    ]);
    let agent = build(model.clone(), std::slice::from_ref(&ask))
        .hook(Arc::new(AskHook))
        .build()
        .unwrap();
    let mut state = agent.new_state(SYSTEM, vec![Message::user("go")]);
    agent
        .run_loop(&mut state, &mut Inbox::detached())
        .await
        .unwrap();
    let Phase::AwaitingApproval { decisions, .. } = &state.phase else {
        panic!("{:?}", state.phase)
    };
    assert!(matches!(
        decisions["c1"],
        ToolDecision::NeedUserInput { .. }
    ));

    // Serialize / restore like a restart, then answer. An approval does not resolve an input.
    let mut state = AgentState::from_parts(state.into_parts()).unwrap();
    let (tx, mut inbox) = Inbox::channel(8);
    tx.send(LoopInput::Approval {
        call_id: "c1".into(),
        decision: ApprovalDecision::Approve,
    })
    .await
    .unwrap();
    tx.send(LoopInput::Approval {
        call_id: "c1".into(),
        decision: ApprovalDecision::Answer {
            content: "the blue one".into(),
        },
    })
    .await
    .unwrap();
    drop(tx);
    agent.run_loop(&mut state, &mut inbox).await.unwrap();
    let r = results(&state.messages()[2])[0];
    assert_eq!(r.content, "the blue one");
    assert!(!r.is_error);
    assert_eq!(
        ask.runs.load(std::sync::atomic::Ordering::SeqCst),
        0,
        "the tool never runs"
    );
    assert_eq!(state.phase, Phase::Idle);
}

#[tokio::test]
async fn explicit_skills_are_deduplicated_per_epoch_and_reset_by_compaction() {
    use crate::skill_inject::active_skill_block;
    let model = MockModel::new(vec![
        MockTurn::text("a", 10),
        MockTurn::text("b", 10),
        MockTurn::text("c", 10),
    ]);
    let (ev_tx, mut ev_rx) = mpsc::channel(1024);
    let agent = Arc::new(
        build(model.clone(), &[])
            .compactor(Arc::new(RuleBasedCompactor {
                max_chars_per_message: 100,
            }))
            .events(ev_tx)
            .build()
            .unwrap(),
    );
    let with_skill = |t: &str| {
        Message::user(format!(
            "{}\n\n{t}",
            active_skill_block("review", "REVIEW BODY")
        ))
    };
    let state = agent.new_state(SYSTEM, vec![]);
    let (tx, inbox) = Inbox::channel(8);
    let handle = spawn_loop(agent.clone(), state, inbox);

    tx.send(LoopInput::Steer(with_skill("one"))).await.unwrap();
    wait_for(&mut ev_rx, |e| matches!(e, AgentEvent::Completed)).await;
    tx.send(LoopInput::Steer(with_skill("two"))).await.unwrap();
    wait_for(&mut ev_rx, |e| matches!(e, AgentEvent::Completed)).await;
    tx.send(LoopInput::Compact).await.unwrap();
    wait_for(&mut ev_rx, |e| matches!(e, AgentEvent::Compacted { .. })).await;
    tx.send(LoopInput::Steer(with_skill("three")))
        .await
        .unwrap();
    wait_for(&mut ev_rx, |e| matches!(e, AgentEvent::Completed)).await;
    drop(tx);
    let (state, r) = join(handle).await;
    r.unwrap();

    let texts: Vec<String> = state.messages().iter().map(Message::text).collect();
    assert!(
        texts[0].contains("REVIEW BODY"),
        "first invocation loads the body"
    );
    assert!(
        texts[2].starts_with("<skill_already_loaded name=\"review\" />"),
        "{}",
        texts[2]
    );
    assert!(!texts[2].contains("REVIEW BODY"));
    assert!(
        texts[4].contains("REVIEW BODY"),
        "compaction resets the epoch"
    );
    assert_eq!(state.loaded_skills.iter().collect::<Vec<_>>(), ["review"]);

    // Earlier requests are a byte-identical prefix of later ones (only the tail grows).
    let reqs = model.requests();
    assert_prefix_stable(&reqs[0], &reqs[1]);
    // And the set survives a persist/restore.
    let restored = AgentState::from_parts(state.into_parts()).unwrap();
    assert!(restored.loaded_skills.contains("review"));
}
