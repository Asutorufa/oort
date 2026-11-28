//! Subagents, explicit skills, builtin tools (approvals, questions, plan, memory) and commands.
//! The mock model calls tools from `@<tool> {json}` lines of the last user message.

mod common;

use std::sync::Arc;
use std::time::{Duration, Instant};

use common::*;
use oort_server::subagent::SubagentLimits;
use oort_storage::SqliteSessionStore;
use serde_json::{Value, json};

fn at(tool: &str, args: Value) -> String {
    format!("@{tool} {args}")
}

fn tool_results(d: &Value) -> Vec<Value> {
    results(d)
}

async fn session(
    s: &Server,
    groups: &[&str],
    ws: Option<&std::path::Path>,
    policy: &str,
) -> String {
    s.create(json!({
        "provider_id": "mock", "builtin_tools": false, "widgets": false,
        "builtin_groups": groups, "workspace_root": ws.map(|p| p.display().to_string()),
        "approval_policy": policy,
    }))
    .await
}

fn limits(max_turns: u32) -> impl FnOnce(oort_server::Registry) -> oort_server::Registry {
    move |r| {
        r.with_subagent_limits(SubagentLimits {
            max_depth: 1,
            default_max_turns: max_turns,
            timeout: Duration::from_secs(20),
        })
    }
}

#[tokio::test]
async fn subagents_run_in_parallel_read_only_and_report_back() {
    let ws = temp_dir("sub-ws");
    std::fs::write(ws.join("a.txt"), "ALPHA CONTENT").unwrap();
    std::fs::write(ws.join("b.txt"), "BETA CONTENT").unwrap();
    let store = Arc::new(SqliteSessionStore::in_memory().await.unwrap());
    let s = Server::start_with(store, &ws, vec![], limits(6)).await;
    let id = session(
        &s,
        &["subagent", "workspace", "utilities"],
        Some(&ws),
        "auto_all",
    )
    .await;
    let d = s.get(&format!("/sessions/{id}")).await;
    assert!(tool_names(&d).contains(&"spawn_subagent".to_string()));

    let events = reqwest::get(format!("{}/sessions/{id}/events", s.base))
        .await
        .unwrap();
    // Each child reads a file slowly (the mock streams "slow" answers word by word).
    let child = |f: &str| {
        at(
            "spawn_subagent",
            json!({ "task": format!("{}\nslow", at("read_file", json!({ "path": f }))), "profile": "explorer" }),
        )
    };
    let started = Instant::now();
    s.steer(&id, &format!("{}\n{}", child("a.txt"), child("b.txt")))
        .await;
    let d = s
        .wait_for(&id, |d| status(d) == "idle" && tool_results(d).len() == 2)
        .await;
    let elapsed = started.elapsed();

    let rs = tool_results(&d);
    assert!(rs.iter().all(|r| r["is_error"] == false), "{rs:?}");
    assert!(
        rs[0]["content"].as_str().unwrap().contains("ALPHA CONTENT"),
        "{}",
        rs[0]["content"]
    );
    assert!(rs[1]["content"].as_str().unwrap().contains("BETA CONTENT"));
    // Children are hidden from the list, visible as children, and recorded with lineage.
    assert_eq!(s.get("/sessions").await.as_array().unwrap().len(), 1);
    let kids = s.get(&format!("/sessions/{id}/children")).await;
    assert_eq!(kids.as_array().unwrap().len(), 2);
    for k in kids.as_array().unwrap() {
        assert_eq!(k["lineage"]["parent_session_id"], id.as_str());
        assert_eq!(k["lineage"]["depth"], 1);
        let kd = s
            .get(&format!("/sessions/{}", k["id"].as_str().unwrap()))
            .await;
        // max_depth 1: children cannot spawn; they only get read-only tools.
        let names = tool_names(&kd);
        assert!(!names.contains(&"spawn_subagent".to_string()), "{names:?}");
        assert!(
            !names.contains(&"write_file".to_string())
                && !names.contains(&"run_command".to_string())
        );
        assert!(names.contains(&"read_file".to_string()));
    }
    // Parallel: the two slow children overlapped (sequential would take about twice as long).
    let one_child = {
        let kd = s
            .get(format!("/sessions/{}/", kids[0]["id"].as_str().unwrap()).trim_end_matches('/'))
            .await;
        kd["usage"]["per_turn"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["latency_ms"].as_u64().unwrap_or(0))
            .sum::<u64>()
    };
    assert!(
        elapsed.as_millis() < (one_child as u128) * 2 + 400,
        "elapsed {elapsed:?}, one child {one_child}ms"
    );

    // Child events are relayed to the parent's stream.
    let ev = read_sse(events, |ev| {
        ev.iter().any(|e| {
            e["event"]["event"] == "subagent" && e["event"]["inner"]["event"] == "tool_started"
        })
    })
    .await;
    let sub: Vec<&Value> = ev
        .iter()
        .filter(|e| e["event"]["event"] == "subagent")
        .collect();
    assert!(
        sub.iter()
            .any(|e| e["event"]["inner"]["event"] == "tool_started"),
        "{sub:?}"
    );
    assert!(sub.iter().all(|e| e["event"]["profile"] == "explorer"));
    s.stop().await;
}

#[tokio::test]
async fn subagent_guardrails_deny_writes_and_depth_and_summarize_at_the_turn_limit() {
    let ws = temp_dir("guard-ws");
    let store = Arc::new(SqliteSessionStore::in_memory().await.unwrap());
    let s = Server::start_with(store, &ws, vec![], limits(1)).await;
    let id = session(
        &s,
        &["subagent", "workspace", "utilities"],
        Some(&ws),
        "auto_all",
    )
    .await;

    // fork mode: the child keeps the parent's catalog (prefix reuse) but writes and spawning
    // are refused by the guard; turn limit 1 forces a summary turn.
    let task = format!(
        "{}\n{}",
        at("write_file", json!({ "path": "x.txt", "content": "nope" })),
        at("spawn_subagent", json!({ "task": "deeper" }))
    );
    s.steer(
        &id,
        &at(
            "spawn_subagent",
            json!({ "task": task, "context_mode": "fork" }),
        ),
    )
    .await;
    let d = s
        .wait_for(&id, |d| status(d) == "idle" && !tool_results(d).is_empty())
        .await;
    let summary = tool_results(&d)[0]["content"].as_str().unwrap().to_string();
    assert!(summary.contains("[turn limit reached"), "{summary}");
    assert!(!ws.join("x.txt").exists(), "nothing was written");

    let kid = &s.get(&format!("/sessions/{id}/children")).await[0];
    let kd = s
        .get(&format!("/sessions/{}", kid["id"].as_str().unwrap()))
        .await;
    assert_eq!(
        tool_names(&kd),
        tool_names(&s.get(&format!("/sessions/{id}")).await),
        "fork keeps the catalog"
    );
    let child_results = tool_results(&kd);
    let texts: Vec<&str> = child_results
        .iter()
        .map(|r| r["content"].as_str().unwrap())
        .collect();
    assert!(texts.iter().any(|t| t.contains("Subagent is read-only. Return your proposed changes to the parent agent for user approval.")), "{texts:?}");
    assert!(
        texts.iter().any(|t| t.contains("maximum subagent depth")),
        "{texts:?}"
    );
    // The fork shares the parent's prefix: same system prompt and history before the task.
    assert_eq!(
        kd["system_prompt"],
        s.get(&format!("/sessions/{id}")).await["system_prompt"]
    );
    assert_eq!(kd["raw"][0]["content"], d["raw"][0]["content"]);
    s.stop().await;
}

#[tokio::test]
async fn interrupting_the_parent_cancels_running_subagents() {
    let ws = temp_dir("cancel-ws");
    let store = Arc::new(SqliteSessionStore::in_memory().await.unwrap());
    let s = Server::start_with(store, &ws, vec![], |r| {
        limits(6)(r.with_mock_word_delay(Duration::from_millis(40)))
    })
    .await;
    let id = session(&s, &["subagent", "workspace"], Some(&ws), "auto_all").await;
    s.steer(
        &id,
        &format!(
            "{}\n{}",
            at("spawn_subagent", json!({ "task": "tell me a slow story" })),
            at("spawn_subagent", json!({ "task": "another slow story" }))
        ),
    )
    .await;
    // Wait until both children exist and are streaming, then interrupt the parent.
    for _ in 0..100 {
        if s.get(&format!("/sessions/{id}/children"))
            .await
            .as_array()
            .unwrap()
            .len()
            == 2
        {
            break;
        }
        tokio::time::sleep(Duration::from_millis(30)).await;
    }
    tokio::time::sleep(Duration::from_millis(300)).await;
    let started = Instant::now();
    s.ok(
        "POST",
        &format!("/sessions/{id}/input"),
        Some(json!({ "type": "interrupt", "text": "stop that" })),
    )
    .await;
    let d = s
        .wait_for(&id, |d| status(d) == "idle" && raw_len(d) >= 5)
        .await;
    assert!(
        started.elapsed() < Duration::from_secs(3),
        "cancellation is prompt"
    );
    let rs = tool_results(&d);
    assert_eq!(rs.len(), 2);
    for r in &rs {
        assert_eq!(r["is_error"], true);
        assert!(r["content"].as_str().unwrap().contains("interrupt"), "{r}");
    }
    // Every call is answered before the interrupt message (protocol order).
    let roles: Vec<&str> = d["raw"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["role"].as_str().unwrap())
        .collect();
    assert_eq!(&roles[..5], ["user", "assistant", "tool", "tool", "user"]);
    // Children stopped streaming (their turns never completed).
    for k in s
        .get(&format!("/sessions/{id}/children"))
        .await
        .as_array()
        .unwrap()
    {
        let kd = s
            .get(&format!("/sessions/{}", k["id"].as_str().unwrap()))
            .await;
        assert_eq!(raw_len(&kd), 1, "no assistant reply was committed");
    }
    s.stop().await;
}

#[tokio::test]
async fn explicit_skills_inline_dedupe_and_subagent_mode() {
    let ws = temp_dir("skill-ws");
    let store = Arc::new(SqliteSessionStore::in_memory().await.unwrap());
    let s = Server::start_with(store, &ws, vec![], limits(4)).await;
    s.ok("POST", "/skills", Some(json!({ "name": "review", "description": "Review code", "body": "REVIEW STEPS for $ARGUMENTS", "argument_hint": "<target>" }))).await;
    s.ok("POST", "/skills", Some(json!({ "name": "deep-research", "description": "Research deeply", "body": "RESEARCH STEPS", "execution_mode": "subagent", "subagent_profile": "researcher" }))).await;
    s.ok("POST", "/skills", Some(json!({ "name": "hidden", "description": "Not a command", "body": "x", "user_invocable": false }))).await;
    let id = session(&s, &["utilities"], None, "auto_readonly").await;
    let input_path = format!("/sessions/{id}/input");
    let send = |text: &str, skills: Value| {
        s.send(
            "POST",
            &input_path,
            Some(json!({ "type": "steer", "text": text, "invoked_skills": skills })),
        )
    };

    assert_eq!(
        send("go", json!([{ "name": "review", "arguments": "main.rs" }]))
            .await
            .status(),
        202
    );
    s.wait_for(&id, |d| status(d) == "idle" && raw_len(d) == 2)
        .await;
    assert_eq!(
        send("again", json!([{ "name": "review" }])).await.status(),
        202
    );
    let d = s
        .wait_for(&id, |d| status(d) == "idle" && raw_len(d) == 4)
        .await;
    let first = d["raw"][0]["content"][0]["text"].as_str().unwrap();
    assert!(
        first.contains("<active_skill name=\"review\">")
            && first.contains("REVIEW STEPS for main.rs")
            && first.ends_with("go"),
        "{first}"
    );
    let second = d["raw"][2]["content"][0]["text"].as_str().unwrap();
    assert!(
        second.starts_with("<skill_already_loaded name=\"review\" />")
            && !second.contains("REVIEW STEPS"),
        "{second}"
    );
    assert_eq!(d["loaded_skills"], json!(["review"]));
    // The system prompt never changes; earlier requests are prefixes of later ones.
    let turns = d["usage"]["per_turn"].as_array().unwrap();
    assert_eq!(
        turns[0]["request"]["hash"]["system"],
        turns[1]["request"]["hash"]["system"]
    );
    assert_eq!(
        turns[0]["request"]["hash"]["tools"],
        turns[1]["request"]["hash"]["tools"]
    );
    assert!(turns[1]["cache_hit_rate"].as_f64().unwrap() > 0.4);

    assert_eq!(send("x", json!([{ "name": "hidden" }])).await.status(), 400);
    assert_eq!(send("x", json!([{ "name": "nope" }])).await.status(), 400);

    // Compaction resets the epoch: the next invocation loads the body again. (Two more turns
    // first: compaction keeps the most recent messages.)
    for (i, t) in ["filler one", "filler two"].iter().enumerate() {
        s.steer(&id, t).await;
        s.wait_for(&id, |d| status(d) == "idle" && raw_len(d) == 6 + 2 * i)
            .await;
    }
    s.ok(
        "POST",
        &format!("/sessions/{id}/input"),
        Some(json!({ "type": "compact" })),
    )
    .await;
    s.wait_for(&id, |d| !d["checkpoints"].as_array().unwrap().is_empty())
        .await;
    send("third", json!([{ "name": "review" }])).await;
    let d = s
        .wait_for(&id, |d| status(d) == "idle" && raw_len(d) == 10)
        .await;
    assert!(
        d["raw"][8]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("REVIEW STEPS")
    );
    assert_eq!(d["loaded_skills"], json!(["review"]));

    // Subagent mode: an isolated child follows the skill; its report arrives as a message.
    send("about rust", json!([{ "name": "deep-research" }])).await;
    let d = s
        .wait_for(&id, |d| status(d) == "idle" && raw_len(d) == 12)
        .await;
    let msg = d["raw"][10]["content"][0]["text"].as_str().unwrap();
    assert!(
        msg.starts_with("<subagent_result skill=\"deep-research\"") && msg.ends_with("about rust"),
        "{msg}"
    );
    let kids = s.get(&format!("/sessions/{id}/children")).await;
    let kid = kids.as_array().unwrap().last().unwrap();
    assert!(
        kid["lineage"]["parent_tool_call_id"]
            .as_str()
            .unwrap()
            .starts_with("skill:")
    );
    let kd = s
        .get(&format!("/sessions/{}", kid["id"].as_str().unwrap()))
        .await;
    assert!(
        kd["raw"][0]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("RESEARCH STEPS")
    );
    s.stop().await;
}

#[tokio::test]
async fn forced_approvals_previews_and_frozen_tools() {
    let ws = temp_dir("force-ws");
    std::fs::write(ws.join("f.txt"), "old line\n").unwrap();
    let store = Arc::new(SqliteSessionStore::in_memory().await.unwrap());
    let s = Server::start(store.clone(), &ws, vec![]).await;
    let id = session(&s, &["self_config", "workspace"], Some(&ws), "auto_all").await;
    let frozen = || async {
        let row: (String,) = sqlx::query_as("SELECT frozen_tools FROM sessions WHERE id = ?")
            .bind(&id)
            .fetch_one(store.pool())
            .await
            .unwrap();
        row.0
    };
    let before = frozen().await;
    let input_path = format!("/sessions/{id}/input");
    let approve = |call: Value| {
        s.ok(
            "POST",
            &input_path,
            Some(json!({ "type": "approval", "call_id": call, "decision": "approve" })),
        )
    };

    // run_command always asks, even with auto_all; the card shows the command.
    s.steer(
        &id,
        &at("run_command", json!({ "command": "echo from-oort" })),
    )
    .await;
    let d = s.wait_for(&id, |d| status(d) == "needs_approval").await;
    let p = &d["pending_approvals"][0];
    assert_eq!(p["kind"], "approval");
    assert_eq!(p["preview"]["kind"], "command");
    assert_eq!(p["preview"]["command"], "echo from-oort");
    approve(p["call"]["id"].clone()).await;
    let d = s.wait_for(&id, |d| status(d) == "idle").await;
    assert!(
        tool_results(&d)[0]["content"]
            .as_str()
            .unwrap()
            .contains("from-oort")
    );

    // write_file follows the policy (auto_all → no question).
    s.steer(
        &id,
        &at(
            "write_file",
            json!({ "path": "f.txt", "old_string": "old", "new_string": "new" }),
        ),
    )
    .await;
    s.wait_for(&id, |d| status(d) == "idle" && tool_results(d).len() == 2)
        .await;
    assert_eq!(
        std::fs::read_to_string(ws.join("f.txt")).unwrap(),
        "new line\n"
    );
    // Under always_ask the card carries a diff preview.
    s.ok(
        "PATCH",
        &format!("/sessions/{id}"),
        Some(json!({ "approval_policy": "always_ask" })),
    )
    .await;
    s.steer(
        &id,
        &at(
            "write_file",
            json!({ "path": "f.txt", "content": "replaced\n" }),
        ),
    )
    .await;
    let d = s.wait_for(&id, |d| status(d) == "needs_approval").await;
    let diff = d["pending_approvals"][0]["preview"]["diff"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(
        diff.contains("-new line") && diff.contains("+replaced"),
        "{diff}"
    );
    s.ok("POST", &format!("/sessions/{id}/input"), Some(json!({ "type": "approval", "call_id": d["pending_approvals"][0]["call"]["id"], "decision": "deny", "reason": "no" }))).await;
    s.wait_for(&id, |d| status(d) == "idle").await;
    s.ok(
        "PATCH",
        &format!("/sessions/{id}"),
        Some(json!({ "approval_policy": "auto_all" })),
    )
    .await;

    // configure_mcp_server: forced approval, masked preview, saved for NEW sessions only.
    s.steer(
        &id,
        &at(
            "configure_mcp_server",
            json!({
                "name": "added", "transport": "streamable_http", "url": "http://127.0.0.1:9/mcp",
                "headers": { "authorization": "Bearer abcdefghijklmnop" }, "test": false
            }),
        ),
    )
    .await;
    let d = s.wait_for(&id, |d| status(d) == "needs_approval").await;
    let p = &d["pending_approvals"][0];
    assert_eq!(p["preview"]["headers"]["authorization"], "Bear…mnop");
    approve(p["call"]["id"].clone()).await;
    let d = s
        .wait_for(&id, |d| status(d) == "idle" && tool_results(d).len() == 4)
        .await;
    let out: Value =
        serde_json::from_str(tool_results(&d)[3]["content"].as_str().unwrap()).unwrap();
    assert_eq!(out["action"], "suggest_new_session");
    let mid = out["mcp_server_id"].as_str().unwrap();
    assert!(
        s.get("/mcp-servers")
            .await
            .as_array()
            .unwrap()
            .iter()
            .any(|m| m["id"] == mid)
    );
    assert_eq!(
        frozen().await,
        before,
        "the running session's tools never change"
    );
    assert!(!tool_names(&d).iter().any(|n| n.starts_with("added__")));

    // save_skill also always asks.
    s.steer(
        &id,
        &at(
            "save_skill",
            json!({ "name": "deploy", "description": "Deploy safely", "body": "1. test\n2. ship" }),
        ),
    )
    .await;
    let d = s.wait_for(&id, |d| status(d) == "needs_approval").await;
    assert_eq!(d["pending_approvals"][0]["preview"]["kind"], "skill");
    approve(d["pending_approvals"][0]["call"]["id"].clone()).await;
    s.wait_for(&id, |d| status(d) == "idle" && tool_results(d).len() == 5)
        .await;
    assert!(
        s.get("/skills")
            .await
            .as_array()
            .unwrap()
            .iter()
            .any(|k| k["name"] == "deploy")
    );
    s.stop().await;
}

#[tokio::test]
async fn ask_user_pauses_survives_restart_and_resumes_with_the_answer() {
    let ws = temp_dir("ask-ws");
    let store = Arc::new(SqliteSessionStore::in_memory().await.unwrap());
    let s = Server::start(store.clone(), &ws, vec![]).await;
    let id = session(&s, &["core_ux"], None, "auto_all").await;
    let events = reqwest::get(format!("{}/sessions/{id}/events", s.base))
        .await
        .unwrap();
    s.steer(&id, &at("update_plan", json!({ "steps": [{ "id": "1", "title": "Ask", "status": "in_progress" }, { "id": "2", "title": "Do", "status": "pending" }] }))).await;
    let ev = read_sse(events, |ev| {
        ev.iter().any(|e| e["event"]["event"] == "plan_updated")
    })
    .await;
    let plan = ev
        .iter()
        .find(|e| e["event"]["event"] == "plan_updated")
        .unwrap();
    assert_eq!(plan["event"]["steps"][0]["status"], "in_progress");
    s.wait_for(&id, |d| status(d) == "idle").await;

    s.steer(
        &id,
        &at(
            "ask_user",
            json!({ "question": "Which color?", "kind": "single", "options": ["red", "blue"] }),
        ),
    )
    .await;
    let d = s.wait_for(&id, |d| status(d) == "needs_approval").await;
    let p = &d["pending_approvals"][0];
    assert_eq!(p["kind"], "input");
    assert_eq!(p["call"]["arguments"]["options"], json!(["red", "blue"]));
    let call_id = p["call"]["id"].clone();
    s.stop().await;

    // Restart: the question is still pending; answering resumes the turn.
    let s = Server::start(store, &ws, vec![]).await;
    assert_eq!(
        s.get(&format!("/sessions/{id}")).await["pending_approvals"][0]["kind"],
        "input"
    );
    s.ok("POST", &format!("/sessions/{id}/input"), Some(json!({ "type": "approval", "call_id": call_id, "decision": "answer", "content": "blue" }))).await;
    let d = s.wait_for(&id, |d| status(d) == "idle").await;
    let answer = tool_results(&d)
        .into_iter()
        .find(|r| r["name"] == "ask_user")
        .unwrap_or_else(|| panic!("{}", d["raw"]));
    assert_eq!(answer["content"], "blue");
    assert_eq!(answer["is_error"], false);
    s.stop().await;
}

#[tokio::test]
async fn memories_through_tools_and_api() {
    let ws = temp_dir("mem-ws");
    let store = Arc::new(SqliteSessionStore::in_memory().await.unwrap());
    let s = Server::start(store, &ws, vec![]).await;
    let id = session(&s, &["memory"], None, "auto_all").await;
    s.steer(&id, &at("memory_save", json!({ "title": "返答スタイル", "content": "ユーザーは簡潔な箇条書きを好む", "tags": ["prefs"] }))).await;
    s.wait_for(&id, |d| status(d) == "idle" && !tool_results(d).is_empty())
        .await;
    s.steer(&id, &at("memory_search", json!({ "query": "箇条書き" })))
        .await;
    let d = s
        .wait_for(&id, |d| status(d) == "idle" && tool_results(d).len() == 2)
        .await;
    assert!(
        tool_results(&d)[1]["content"]
            .as_str()
            .unwrap()
            .contains("返答スタイル")
    );

    // REST: create, search (Chinese / English / tags), update, delete.
    let m = s.ok("POST", "/memories", Some(json!({ "title": "部署", "content": "使用蓝绿发布策略", "tags": ["ops"], "scope": "proj" }))).await;
    let mid = m["id"].as_str().unwrap();
    let hits = s
        .get("/memories?q=%E8%93%9D%E7%BB%BF%E5%8F%91%E5%B8%83")
        .await;
    assert_eq!(hits.as_array().unwrap().len(), 1);
    assert_eq!(
        s.get("/memories?tag=ops").await.as_array().unwrap().len(),
        1
    );
    assert_eq!(s.get("/memories").await.as_array().unwrap().len(), 2);
    s.ok(
        "PUT",
        &format!("/memories/{mid}"),
        Some(json!({ "content": "rolling deploys with canaries" })),
    )
    .await;
    assert_eq!(
        s.get("/memories?q=canaries")
            .await
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert!(
        s.get("/memories?q=%E8%93%9D%E7%BB%BF%E5%8F%91%E5%B8%83")
            .await
            .as_array()
            .unwrap()
            .is_empty()
    );
    s.ok("DELETE", &format!("/memories/{mid}"), None).await;
    assert_eq!(s.get("/memories").await.as_array().unwrap().len(), 1);
    assert_eq!(
        s.send("POST", "/memories", Some(json!({ "title": "" })))
            .await
            .status(),
        400
    );
    s.stop().await;
}

#[tokio::test]
async fn commands_are_listed_and_run_deterministically() {
    let ws = temp_dir("cmd-ws");
    let store = Arc::new(SqliteSessionStore::in_memory().await.unwrap());
    let s = Server::start(store, &ws, vec![]).await;
    s.ok("POST", "/skills", Some(json!({ "name": "triage", "description": "Triage", "body": "b", "execution_mode": "subagent", "argument_hint": "<incident>" }))).await;
    s.ok(
        "POST",
        "/skills",
        Some(json!({ "name": "secret", "description": "x", "body": "b", "user_invocable": false })),
    )
    .await;
    let cmds = s.get("/commands").await;
    let names: Vec<&str> = cmds
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["name"].as_str().unwrap())
        .collect();
    for n in ["compact", "approval", "stop", "export", "new", "triage"] {
        assert!(names.contains(&n), "{names:?}");
    }
    assert!(!names.contains(&"secret"));
    let triage = cmds
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["name"] == "triage")
        .unwrap();
    assert_eq!(
        (
            triage["kind"].clone(),
            triage["subagent"].clone(),
            triage["argument_hint"].clone()
        ),
        (json!("skill"), json!(true), json!("<incident>"))
    );

    let id = session(&s, &["utilities", "memory"], None, "auto_readonly").await;
    let run = |name: &str, args: Option<&str>| {
        s.ok(
            "POST",
            "/commands/run",
            Some(json!({ "session_id": id, "name": name, "arguments": args })),
        )
    };
    run("approval", Some("ask")).await;
    assert_eq!(
        s.get(&format!("/sessions/{id}")).await["approval_policy"],
        "always_ask"
    );
    let ex = run("export", None).await;
    assert_eq!(
        ex["effect"]["download"],
        format!("/api/sessions/{id}/export")
    );
    let new = run("new", None).await;
    let nid = new["effect"]["navigate_session"].as_str().unwrap();
    let nd = s.get(&format!("/sessions/{nid}")).await;
    assert_eq!(
        nd["lineage"]["builtin_groups"],
        json!(["memory", "utilities"]),
        "same tools as the current chat"
    );
    assert_eq!(run("stop", None).await["message"], "Stopped");
    assert_eq!(
        s.send(
            "POST",
            "/commands/run",
            Some(json!({ "name": "approval", "arguments": "ask" }))
        )
        .await
        .status(),
        400,
        "needs a session"
    );
    assert_eq!(
        s.send(
            "POST",
            "/commands/run",
            Some(json!({ "session_id": id, "name": "approval", "arguments": "sometimes" }))
        )
        .await
        .status(),
        400
    );
    assert_eq!(
        s.send("POST", "/commands/run", Some(json!({ "name": "bogus" })))
            .await
            .status(),
        404
    );

    // Workspace needs a root; unknown groups are rejected.
    assert_eq!(
        s.send(
            "POST",
            "/sessions",
            Some(json!({ "provider_id": "mock", "builtin_groups": ["workspace"] }))
        )
        .await
        .status(),
        400
    );
    assert_eq!(
        s.send(
            "POST",
            "/sessions",
            Some(json!({ "provider_id": "mock", "builtin_groups": ["bogus"] }))
        )
        .await
        .status(),
        400
    );
    s.stop().await;
}
