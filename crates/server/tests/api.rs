//! End-to-end API tests: real HTTP server, in-memory SQLite, mock provider, built-in tools and
//! an in-process Streamable HTTP MCP server.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use futures_util::StreamExt;
use oort_agent::{AgentState, NewSession, SessionStore};
use oort_server::{Registry, SessionManager, router};
use oort_storage::SqliteSessionStore;
use serde_json::{Value, json};

// ----- harness -------------------------------------------------------------------------------

struct Server {
    base: String,
    manager: Arc<SessionManager>,
    task: tokio::task::JoinHandle<()>,
}

impl Server {
    async fn start(store: Arc<SqliteSessionStore>, workspace: &std::path::Path) -> Self {
        let builtin = Arc::new(oort_server::tools::builtin_executor(workspace));
        let registry = Arc::new(
            Registry::new(store.clone(), builtin).with_mock_word_delay(Duration::from_millis(5)),
        );
        let manager = SessionManager::new(store, registry);
        let app = router(manager.clone(), None);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}/api", listener.local_addr().unwrap());
        let task = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        Self {
            base,
            manager,
            task,
        }
    }

    /// Simulates a process restart: loops are cancelled, the HTTP server goes away.
    async fn stop(self) {
        self.manager.shutdown().await;
        self.task.abort();
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    async fn send(&self, method: &str, path: &str, body: Option<Value>) -> reqwest::Response {
        let mut req =
            reqwest::Client::new().request(method.parse().unwrap(), format!("{}{path}", self.base));
        if let Some(b) = body {
            req = req.json(&b);
        }
        req.send().await.unwrap()
    }

    async fn post(&self, path: &str, body: Value) -> reqwest::Response {
        self.send("POST", path, Some(body)).await
    }

    /// Request that must succeed; returns the JSON body (or `null` for 204).
    async fn ok(&self, method: &str, path: &str, body: Option<Value>) -> Value {
        let r = self.send(method, path, body).await;
        let status = r.status();
        let text = r.text().await.unwrap();
        assert!(status.is_success(), "{method} {path}: {status} {text}");
        if text.is_empty() {
            Value::Null
        } else {
            serde_json::from_str(&text).unwrap()
        }
    }

    async fn get(&self, path: &str) -> Value {
        self.ok("GET", path, None).await
    }

    async fn create_with(&self, body: Value) -> String {
        self.ok("POST", "/sessions", Some(body)).await["id"]
            .as_str()
            .unwrap()
            .to_string()
    }

    async fn create(&self, message: &str) -> String {
        self.create_with(json!({
            "provider_id": "mock", "system_prompt": "You are a test agent.", "message": message
        }))
        .await
    }

    /// Polls the snapshot until `pred` holds.
    async fn wait_for(&self, id: &str, pred: impl Fn(&Value) -> bool) -> Value {
        for _ in 0..250 {
            let d = self.get(&format!("/sessions/{id}")).await;
            if pred(&d) {
                return d;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        panic!(
            "condition not reached: {}",
            self.get(&format!("/sessions/{id}")).await
        );
    }

    async fn steer(&self, id: &str, text: &str) {
        let r = self
            .post(
                &format!("/sessions/{id}/input"),
                json!({ "type": "steer", "text": text }),
            )
            .await;
        assert_eq!(r.status(), 202);
    }
}

fn workspace() -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "oort-server-test-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("hello.txt"), "hi there").unwrap();
    dir
}

fn status(d: &Value) -> &str {
    d["status"].as_str().unwrap()
}

fn tool_names(d: &Value) -> Vec<String> {
    d["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap().to_string())
        .collect()
}

/// Reads SSE `data:` payloads (and `id:`s) until `done` returns true (or a timeout).
async fn read_sse(
    resp: reqwest::Response,
    mut done: impl FnMut(&[Value]) -> bool,
) -> (Vec<Value>, Vec<String>) {
    let mut body = resp.bytes_stream();
    let (mut buf, mut events, mut ids) = (String::new(), Vec::new(), Vec::new());
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while !done(&events) && tokio::time::Instant::now() < deadline {
        let Ok(Some(chunk)) = tokio::time::timeout(Duration::from_secs(5), body.next()).await
        else {
            break;
        };
        buf.push_str(&String::from_utf8_lossy(&chunk.unwrap()));
        while let Some(end) = buf.find("\n\n") {
            let frame: String = buf.drain(..end + 2).collect();
            for line in frame.lines() {
                if let Some(d) = line.strip_prefix("data: ") {
                    events.push(serde_json::from_str(d).unwrap());
                } else if let Some(i) = line.strip_prefix("id: ") {
                    ids.push(i.to_string());
                }
            }
        }
    }
    (events, ids)
}

// ----- in-process Streamable HTTP MCP server ------------------------------------------------

#[derive(Clone, Default)]
struct McpMock {
    tools: Arc<Mutex<Vec<Value>>>,
    seen_tokens: Arc<Mutex<Vec<String>>>,
}

impl McpMock {
    fn set_tools(&self, names: &[&str]) {
        *self.tools.lock().unwrap() = names
            .iter()
            .map(|n| {
                json!({
                    "name": n,
                    "description": format!("{n} tool"),
                    "inputSchema": { "type": "object", "properties": { "text": { "type": "string" } } },
                    "annotations": { "readOnlyHint": *n != "write" },
                })
            })
            .collect();
    }

    async fn start(&self) -> String {
        let mock = self.clone();
        let app = axum::Router::new().route(
            "/mcp",
            axum::routing::post(move |headers: HeaderMap, body: String| {
                let mock = mock.clone();
                async move { mock.handle(&headers, &body) }
            })
            .delete(|| async { StatusCode::OK }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/mcp", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        url
    }

    fn handle(&self, headers: &HeaderMap, body: &str) -> axum::response::Response {
        if let Some(t) = headers.get("x-token") {
            self.seen_tokens
                .lock()
                .unwrap()
                .push(t.to_str().unwrap().to_string());
        }
        let req: Value = serde_json::from_str(body).unwrap();
        let Some(id) = req.get("id").cloned() else {
            return StatusCode::ACCEPTED.into_response(); // notification
        };
        let params = &req["params"];
        let result = match req["method"].as_str().unwrap() {
            "initialize" => json!({
                "protocolVersion": params["protocolVersion"],
                "capabilities": { "tools": {} },
                "serverInfo": { "name": "http-mock", "version": "1.2.3" }
            }),
            "tools/list" => json!({ "tools": *self.tools.lock().unwrap() }),
            "tools/call" => {
                let name = params["name"].as_str().unwrap();
                let text = params["arguments"]["text"].as_str().unwrap_or_default();
                match name {
                    "fail" => {
                        json!({ "content": [{ "type": "text", "text": "it broke" }], "isError": true })
                    }
                    "boom" => {
                        return axum::Json(json!({ "jsonrpc": "2.0", "id": id,
                            "error": { "code": -32602, "message": "bad arguments" } }))
                        .into_response();
                    }
                    other => json!({
                        "content": [{ "type": "text", "text": format!("{other}: {text}") }],
                        "structuredContent": { "echo": text },
                    }),
                }
            }
            other => panic!("unexpected method {other}"),
        };
        axum::Json(json!({ "jsonrpc": "2.0", "id": id, "result": result })).into_response()
    }
}

async fn add_mcp(s: &Server, name: &str, url: &str) -> String {
    s.ok(
        "POST",
        "/mcp-servers",
        Some(json!({ "name": name, "transport": "streamable_http", "url": url })),
    )
    .await["id"]
        .as_str()
        .unwrap()
        .to_string()
}

// ----- sessions ------------------------------------------------------------------------------

#[tokio::test]
async fn create_run_tools_and_report_usage() {
    let store = Arc::new(SqliteSessionStore::in_memory().await.unwrap());
    let ws = workspace();
    let s = Server::start(store, &ws).await;

    let id = s.create("please list the files").await;
    let d = s.wait_for(&id, |d| status(d) == "idle").await;
    let roles: Vec<&str> = d["raw"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["role"].as_str().unwrap())
        .collect();
    assert_eq!(roles, ["user", "assistant", "tool", "assistant"]);
    assert!(
        d["raw"][2]["content"][0]["content"]
            .as_str()
            .unwrap()
            .contains("hello.txt")
    );
    assert_eq!(d["tools"].as_array().unwrap().len(), 4);
    assert_eq!(d["active"], true);
    assert_eq!(d["settings"]["provider_id"], "mock");
    assert_eq!(d["approval_policy"], "auto_readonly");

    // Second turn via Steer: the cache-hit rate reflects the reused prefix.
    s.steer(&id, "thanks").await;
    let d = s
        .wait_for(&id, |d| {
            d["raw"].as_array().unwrap().len() == 6 && status(d) == "idle"
        })
        .await;
    let turns = d["usage"]["per_turn"].as_array().unwrap();
    assert_eq!(turns.len(), 3);
    assert!(
        turns[2]["cache_hit_rate"].as_f64().unwrap() > 0.5,
        "{turns:?}"
    );
    // Per-call debug info: timing and a request summary with stable prefix hashes.
    for t in turns {
        assert!(t["ttft_ms"].is_u64() && t["latency_ms"].is_u64(), "{t}");
    }
    assert_eq!(turns[2]["request"]["messages"], 5);
    assert_eq!(
        turns[0]["request"]["hash"]["tools"],
        turns[2]["request"]["hash"]["tools"]
    );
    assert_eq!(
        turns[0]["request"]["hash"]["system"],
        turns[2]["request"]["hash"]["system"]
    );
    assert!(d["usage"]["cache_hit_rate"].as_f64().unwrap() > 0.0);
    assert!(d["context"]["tokens"].as_u64().unwrap() > 0);

    let list = s.get("/sessions").await;
    assert_eq!(list[0]["id"], id.as_str());
    assert_eq!(list[0]["message_count"], 6);

    // Manual compaction is visible in both checkpoints and the active view.
    s.post(
        &format!("/sessions/{id}/input"),
        json!({ "type": "compact" }),
    )
    .await;
    let d = s
        .wait_for(&id, |d| !d["checkpoints"].as_array().unwrap().is_empty())
        .await;
    let from = d["active_view"]["from_seq"].as_u64().unwrap() as usize;
    assert!(from > 0);
    assert_eq!(
        d["active_view"]["messages"].as_array().unwrap().len(),
        6 - from + 1
    );
    assert_eq!(
        d["raw"].as_array().unwrap().len(),
        6,
        "raw history is untouched"
    );
    s.stop().await;
}

#[tokio::test]
async fn approval_survives_restart() {
    let store = Arc::new(SqliteSessionStore::in_memory().await.unwrap());
    let ws = workspace();
    let s = Server::start(store.clone(), &ws).await;
    let id = s.create("note remember the milk").await;
    let d = s.wait_for(&id, |d| status(d) == "needs_approval").await;
    let call_id = d["pending_approvals"][0]["call"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    assert_eq!(d["pending_approvals"][0]["call"]["name"], "write_note");
    assert_eq!(d["phase"]["phase"], "awaiting_approval");
    s.stop().await;

    // New process, same database: load the paused session and approve it.
    let s = Server::start(store, &ws).await;
    let d = s.get(&format!("/sessions/{id}")).await;
    assert_eq!(status(&d), "needs_approval");
    assert_eq!(d["active"], false);
    let r = s
        .post(
            &format!("/sessions/{id}/input"),
            json!({ "type": "approval", "call_id": call_id, "decision": "approve" }),
        )
        .await;
    assert_eq!(r.status(), 202);
    let d = s.wait_for(&id, |d| status(d) == "idle").await;
    let result = &d["raw"][2]["content"][0];
    assert_eq!(result["is_error"], false);
    assert!(
        std::fs::read_to_string(ws.join("oort-notes.md"))
            .unwrap()
            .contains("remember the milk")
    );

    // Deny path on a second session.
    let id2 = s.create("note do not write this").await;
    let d = s.wait_for(&id2, |d| status(d) == "needs_approval").await;
    let call_id = d["pending_approvals"][0]["call"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    s.post(
        &format!("/sessions/{id2}/input"),
        json!({ "type": "approval", "call_id": call_id, "decision": "deny", "reason": "nope" }),
    )
    .await;
    let d = s.wait_for(&id2, |d| status(d) == "idle").await;
    assert_eq!(d["raw"][2]["content"][0]["is_error"], true);
    assert!(
        d["raw"][2]["content"][0]["content"]
            .as_str()
            .unwrap()
            .contains("nope")
    );
    s.stop().await;
}

#[tokio::test]
async fn sse_streams_events_and_interrupt_works() {
    let store = Arc::new(SqliteSessionStore::in_memory().await.unwrap());
    let ws = workspace();
    let s = Server::start(store, &ws).await;
    let id = s.create("").await;

    let resp = reqwest::get(format!("{}/sessions/{id}/events", s.base))
        .await
        .unwrap();
    assert_eq!(resp.headers()["content-type"], "text/event-stream");
    let mut body = resp.bytes_stream();

    // A long, slow answer; interrupt it mid-stream.
    s.steer(&id, "tell me a slow story").await;
    let mut buf = String::new();
    let mut interrupted = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while tokio::time::Instant::now() < deadline {
        let chunk = tokio::time::timeout(Duration::from_secs(5), body.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        buf.push_str(&String::from_utf8_lossy(&chunk));
        if !interrupted && buf.contains("\"text_delta\"") {
            s.post(
                &format!("/sessions/{id}/input"),
                json!({ "type": "interrupt", "text": "stop, be brief" }),
            )
            .await;
            interrupted = true;
        }
        if buf.contains("\"during\":\"llm_stream\"") && buf.matches("\"completed\"").count() >= 1 {
            break;
        }
    }
    assert!(buf.starts_with("data: {\"kind\":\"hello\""), "{buf}");
    assert!(buf.contains("data: {\"kind\":\"agent\""), "{buf}");
    let boot = s.manager.boot();
    assert!(
        buf.contains(&format!("id: {boot}:1")),
        "SSE ids carry boot and sequence number"
    );
    assert!(buf.contains("\"interrupted\""), "{buf}");

    let d = s.wait_for(&id, |d| status(d) == "idle").await;
    let texts: Vec<String> = d["raw"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| {
            m["content"][0]["text"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
        .collect();
    assert_eq!(texts[0], "tell me a slow story");
    assert_eq!(texts[1], "stop, be brief", "partial output was discarded");
    assert!(texts[2].starts_with("You said: \"stop, be brief\""));

    // Cancel stops the loop; the session can be resumed later.
    assert_eq!(
        s.post(&format!("/sessions/{id}/cancel"), json!({}))
            .await
            .status(),
        202
    );
    let d = s.wait_for(&id, |d| d["active"] == false).await;
    assert_eq!(status(&d), "idle");
    s.steer(&id, "back").await;
    s.wait_for(&id, |d| d["raw"].as_array().unwrap().len() == 5)
        .await;
    s.stop().await;
}

#[tokio::test]
async fn sse_reconnect_replays_missed_events_or_resyncs() {
    let store = Arc::new(SqliteSessionStore::in_memory().await.unwrap());
    let s = Server::start(store, &workspace()).await;
    let id = s.create("").await;
    let url = format!("{}/sessions/{id}/events", s.base);

    let first = reqwest::get(&url).await.unwrap();
    s.steer(&id, "hello there").await;
    let (events, ids) = read_sse(first, |ev| {
        ev.iter()
            .any(|e| e["kind"] == "agent" && e["event"]["event"] == "completed")
    })
    .await;
    assert_eq!(events[0]["kind"], "hello");
    assert_eq!(events[0]["resumed"], false);
    let total = events.len();
    assert!(total > 4, "{events:?}");
    assert_eq!(ids.len(), total, "every envelope carries an id");

    // Reconnect "after" the second envelope: everything after it is replayed in order.
    let resumed = reqwest::Client::new()
        .get(&url)
        .header("Last-Event-ID", &ids[1])
        .send()
        .await
        .unwrap();
    let want = total - 1; // new hello + events[2..]
    let (replay, _) = read_sse(resumed, |ev| ev.len() >= want).await;
    assert_eq!(replay[0]["kind"], "hello");
    assert_eq!(replay[0]["resumed"], true);
    assert_eq!(replay[1..], events[2..], "missed events, same order");

    // An id from another server process cannot be resumed: the client must resync.
    let stale = reqwest::Client::new()
        .get(&url)
        .header("Last-Event-ID", "other-boot:3")
        .send()
        .await
        .unwrap();
    let (ev, _) = read_sse(stale, |ev| !ev.is_empty()).await;
    assert_eq!(ev[0]["resumed"], false);
    assert_eq!(ev[0]["live"]["text"], "");
    s.stop().await;
}

#[tokio::test]
async fn patch_policy_export_and_delete() {
    let store = Arc::new(SqliteSessionStore::in_memory().await.unwrap());
    let s = Server::start(store, &workspace()).await;
    let id = s.create("").await;
    s.wait_for(&id, |d| status(d) == "idle").await;

    // always_ask: even a read-only tool waits for a human.
    let d = s
        .ok(
            "PATCH",
            &format!("/sessions/{id}"),
            Some(json!({ "title": "  renamed ", "approval_policy": "always_ask" })),
        )
        .await;
    assert_eq!(d["title"], "renamed");
    assert_eq!(d["approval_policy"], "always_ask");
    s.steer(&id, "what time is it").await;
    let d = s.wait_for(&id, |d| status(d) == "needs_approval").await;
    assert_eq!(d["pending_approvals"][0]["call"]["name"], "get_time");
    assert!(
        d["pending_approvals"][0]["reason"]
            .as_str()
            .unwrap()
            .contains("always_ask")
    );
    let call_id = d["pending_approvals"][0]["call"]["id"].clone();
    s.post(
        &format!("/sessions/{id}/input"),
        json!({ "type": "approval", "call_id": call_id, "decision": "approve" }),
    )
    .await;
    s.wait_for(&id, |d| status(d) == "idle").await;

    // auto_all while running: a mutating tool runs without asking.
    s.ok(
        "PATCH",
        &format!("/sessions/{id}"),
        Some(json!({ "approval_policy": "auto_all" })),
    )
    .await;
    s.steer(&id, "note no approval needed").await;
    let d = s
        .wait_for(&id, |d| {
            status(d) == "idle" && d["raw"].as_array().unwrap().len() == 8
        })
        .await;
    assert_eq!(d["raw"][6]["content"][0]["is_error"], false);

    let x = s.get(&format!("/sessions/{id}/export")).await;
    assert_eq!(x["format"], "oort.session.v1");
    assert_eq!(x["meta"]["title"], "renamed");
    assert_eq!(x["messages"].as_array().unwrap().len(), 8);
    assert_eq!(x["frozen_tools"].as_array().unwrap().len(), 4);
    assert_eq!(x["llm_calls"].as_array().unwrap().len(), 4);
    assert_eq!(x["provider"]["kind"], "mock");
    assert!(x["provider"].get("api_key").is_none());

    // Deleting an active session: its SSE stream gets `deleted` and ends.
    let events = reqwest::get(format!("{}/sessions/{id}/events", s.base))
        .await
        .unwrap();
    let r = s.send("DELETE", &format!("/sessions/{id}"), None).await;
    assert_eq!(r.status(), 204);
    let (ev, _) = read_sse(events, |ev| ev.iter().any(|e| e["kind"] == "deleted")).await;
    assert!(ev.iter().any(|e| e["kind"] == "deleted"), "{ev:?}");
    assert_eq!(
        s.send("GET", &format!("/sessions/{id}"), None)
            .await
            .status(),
        404
    );
    assert!(s.get("/sessions").await.as_array().unwrap().is_empty());
    s.stop().await;
}

#[tokio::test]
async fn legacy_sessions_still_resume() {
    let store = Arc::new(SqliteSessionStore::in_memory().await.unwrap());
    let ws = workspace();
    let s = Server::start(store.clone(), &ws).await;
    // A session as the pre-workbench console stored it.
    let exec = oort_server::tools::builtin_executor(&ws);
    let frozen = oort_agent::FrozenToolset::freeze(&exec).unwrap();
    let state = AgentState::with_tools("old", vec![], &frozen);
    let id = store
        .create_session(
            NewSession {
                provider: "mock".into(),
                model: "mock".into(),
                config: json!({ "provider": "mock", "model": "mock", "max_context_tokens": 1000, "approval": "mutating" }),
                ..Default::default()
            },
            &state,
        )
        .await
        .unwrap();
    s.steer(&id, "hi").await;
    let d = s.wait_for(&id, |d| status(d) == "idle").await;
    assert_eq!(d["raw"].as_array().unwrap().len(), 2);
    assert!(d["settings"].get("provider_id").is_none());
    s.stop().await;
}

#[tokio::test]
async fn errors_are_json() {
    let store = Arc::new(SqliteSessionStore::in_memory().await.unwrap());
    let s = Server::start(store, &workspace()).await;
    let r = s.send("GET", "/sessions/missing", None).await;
    assert_eq!(r.status(), 404);
    assert!(
        r.json::<Value>().await.unwrap()["error"]
            .as_str()
            .unwrap()
            .contains("missing")
    );
    for body in [
        json!({ "model": "x" }),
        json!({ "provider_id": "nope" }),
        json!({ "provider_id": "mock", "mcp_server_ids": ["missing"] }),
    ] {
        let r = s.post("/sessions", body.clone()).await;
        assert!(
            r.status() == 400 || r.status() == 404,
            "{body}: {}",
            r.status()
        );
    }
    s.stop().await;
}

// ----- providers -----------------------------------------------------------------------------

#[tokio::test]
async fn provider_crud_masks_secrets_and_tests_connectivity() {
    let store = Arc::new(SqliteSessionStore::in_memory().await.unwrap());
    let s = Server::start(store.clone(), &workspace()).await;

    let p = s
        .ok(
            "POST",
            "/providers",
            Some(json!({
                "name": "Gemini", "kind": "gemini", "api_key": "AIzaSyD-0123456789-9x2Q",
                "default_model": "gemini-2.5-flash",
                "extra_headers": { "x-goog-api-key": "abcdefghijklmnopqrstu", "x-org": "acme" }
            })),
        )
        .await;
    let pid = p["id"].as_str().unwrap().to_string();
    assert_eq!(p["api_key"], "AIza…9x2Q");
    assert_eq!(p["api_key_source"], "inline");
    assert_eq!(p["extra_headers"]["x-goog-api-key"], "abcd…rstu");
    assert_eq!(p["extra_headers"]["x-org"], "acme");
    let listed = s.get("/providers").await.to_string();
    assert!(!listed.contains("0123456789") && !listed.contains("efghijklmnop"));

    // PUT with an empty key (and the masked headers echoed back) keeps the secrets.
    let p = s
        .ok(
            "PUT",
            &format!("/providers/{pid}"),
            Some(json!({ "api_key": "", "extra_headers": p["extra_headers"], "default_model": "gemini-2.5-pro" })),
        )
        .await;
    assert_eq!(p["default_model"], "gemini-2.5-pro");
    let stored = oort_agent::ConfigStore::get_provider(store.as_ref(), &pid)
        .await
        .unwrap();
    assert_eq!(stored.api_key.as_deref(), Some("AIzaSyD-0123456789-9x2Q"));
    assert_eq!(
        stored.extra_headers["x-goog-api-key"],
        "abcdefghijklmnopqrstu"
    );

    // env: references are resolved at use time; a missing variable is a clear test failure.
    let t = s
        .ok(
            "POST",
            &format!("/providers/{pid}/test"),
            Some(json!({ "api_key": "env:OORT_TEST_SURELY_UNSET_KEY" })),
        )
        .await;
    assert_eq!(t["ok"], false);
    assert!(
        t["error"]
            .as_str()
            .unwrap()
            .contains("OORT_TEST_SURELY_UNSET_KEY"),
        "{t}"
    );
    // ... and the override was not saved.
    assert_eq!(
        s.get(&format!("/providers/{pid}")).await["api_key"],
        "AIza…9x2Q"
    );

    // Offline provider: always reachable.
    let t = s.ok("POST", "/providers/mock/test", None).await;
    assert_eq!(t["ok"], true, "{t}");
    assert_eq!(t["model_found"], true);
    assert!(t["latency_ms"].is_u64());

    // Unsaved form test, validation, kind is immutable, names are unique.
    let t = s
        .ok(
            "POST",
            "/providers/test",
            Some(json!({ "name": "x", "kind": "openai_compatible", "base_url": "http://127.0.0.1:9/v1" })),
        )
        .await;
    assert_eq!(t["ok"], false);
    let r = s
        .post(
            "/providers",
            json!({ "name": "y", "kind": "openai_compatible" }),
        )
        .await;
    assert_eq!(r.status(), 400);
    let r = s
        .send(
            "PUT",
            &format!("/providers/{pid}"),
            Some(json!({ "kind": "anthropic" })),
        )
        .await;
    assert_eq!(r.status(), 400);
    let r = s
        .post("/providers", json!({ "name": "Gemini", "kind": "gemini" }))
        .await;
    assert_eq!(r.status(), 409, "duplicate name");

    s.ok("DELETE", &format!("/providers/{pid}"), None).await;
    assert_eq!(
        s.send("GET", &format!("/providers/{pid}"), None)
            .await
            .status(),
        404
    );
    s.stop().await;
}

// ----- MCP servers ---------------------------------------------------------------------------

#[tokio::test]
async fn mcp_crud_discovery_and_playground() {
    let store = Arc::new(SqliteSessionStore::in_memory().await.unwrap());
    let s = Server::start(store, &workspace()).await;
    let mock = McpMock::default();
    mock.set_tools(&["echo", "fail", "write", "boom"]);
    let url = mock.start().await;

    // Header values may reference the environment.
    // SAFETY: the variable is unique to this test and only read through `std::env::var`.
    unsafe { std::env::set_var("OORT_TEST_MCP_TOKEN", "t0ken-from-env") };
    let m = s
        .ok(
            "POST",
            "/mcp-servers",
            Some(json!({
                "name": "remote", "transport": "streamable_http", "url": url,
                "headers": { "x-token": "env:OORT_TEST_MCP_TOKEN", "authorization": "Bearer abcdefghijklmnop" }
            })),
        )
        .await;
    let mid = m["id"].as_str().unwrap().to_string();
    assert_eq!(m["headers"]["x-token"], "env:OORT_TEST_MCP_TOKEN");
    assert_eq!(m["headers"]["authorization"], "Bear…mnop");

    let t = s
        .ok("POST", &format!("/mcp-servers/{mid}/test"), None)
        .await;
    assert_eq!(t["ok"], true, "{t}");
    assert_eq!(t["server_info"]["name"], "http-mock");
    assert_eq!(t["server_info"]["version"], "1.2.3");
    let tools: Vec<(&str, bool)> = t["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| {
            (
                t["name"].as_str().unwrap(),
                t["read_only"].as_bool().unwrap(),
            )
        })
        .collect();
    assert_eq!(
        tools,
        [
            ("boom", true),
            ("echo", true),
            ("fail", true),
            ("write", false)
        ],
        "sorted, readOnlyHint mapped"
    );
    assert_eq!(t["tools"][1]["input_schema"]["type"], "object");
    let seen = mock.seen_tokens.lock().unwrap().clone();
    assert!(!seen.is_empty() && seen.iter().all(|t| t == "t0ken-from-env"));

    // Playground: direct calls, no LLM.
    let call_path = format!("/mcp-servers/{mid}/tools/call");
    let call = |name: &str, args: Value| {
        let body = json!({ "tool_name": name, "arguments": args });
        s.ok("POST", &call_path, Some(body))
    };
    let r = call("echo", json!({ "text": "hi" })).await;
    assert_eq!(r["ok"], true);
    assert_eq!(r["is_error"], false);
    assert_eq!(r["text"], "echo: hi");
    assert_eq!(r["content"][0]["type"], "text");
    assert_eq!(r["structured_content"]["echo"], "hi");
    assert!(r["latency_ms"].is_u64());
    let r = call("fail", json!({})).await;
    assert_eq!(r["ok"], true);
    assert_eq!(r["is_error"], true);
    assert_eq!(r["text"], "it broke");
    let r = call("boom", json!({})).await;
    assert_eq!(r["ok"], false);
    assert!(
        r["error"].as_str().unwrap().contains("bad arguments"),
        "{r}"
    );

    // Unreachable server: the test reports it instead of failing the request.
    let dead = add_mcp(&s, "dead", "http://127.0.0.1:9/mcp").await;
    let t = s
        .ok("POST", &format!("/mcp-servers/{dead}/test"), None)
        .await;
    assert_eq!(t["ok"], false);
    assert!(t["error"].is_string());

    // Unsaved test + validation.
    let t = s
        .ok(
            "POST",
            "/mcp-servers/test",
            Some(json!({ "name": "tmp", "transport": "streamable_http", "url": url })),
        )
        .await;
    assert_eq!(t["ok"], true);
    for bad in [
        json!({ "name": "has space", "transport": "stdio", "command": "x" }),
        json!({ "name": "nourl", "transport": "streamable_http" }),
    ] {
        assert_eq!(s.post("/mcp-servers", bad).await.status(), 400);
    }

    // Edit keeps masked secrets; delete removes.
    let m = s
        .ok(
            "PUT",
            &format!("/mcp-servers/{mid}"),
            Some(json!({ "headers": m["headers"], "is_enabled": false })),
        )
        .await;
    assert_eq!(m["is_enabled"], false);
    assert_eq!(m["headers"]["authorization"], "Bear…mnop");
    s.ok("DELETE", &format!("/mcp-servers/{mid}"), None).await;
    assert_eq!(s.get("/mcp-servers").await.as_array().unwrap().len(), 1);
    s.stop().await;
}

#[tokio::test]
async fn frozen_tools_are_unaffected_by_mcp_changes() {
    let store = Arc::new(SqliteSessionStore::in_memory().await.unwrap());
    let s = Server::start(store.clone(), &workspace()).await;
    let mock = McpMock::default();
    mock.set_tools(&["echo", "fail"]);
    let url = mock.start().await;
    let mid = add_mcp(&s, "remote", &url).await;

    let id = s
        .create_with(json!({
            "provider_id": "mock", "mcp_server_ids": [mid], "builtin_tools": false,
            // A vanished tool is scheduled as non-read-only (it would need approval otherwise).
            "approval_policy": "auto_all",
        }))
        .await;
    let d = s.get(&format!("/sessions/{id}")).await;
    assert_eq!(tool_names(&d), ["remote__echo", "remote__fail"]);
    let frozen = || async {
        let row: (String,) = sqlx::query_as("SELECT frozen_tools FROM sessions WHERE id = ?")
            .bind(&id)
            .fetch_one(store.pool())
            .await
            .unwrap();
        row.0
    };
    let before = frozen().await;

    // The server changes its tools, is renamed and re-discovered.
    mock.set_tools(&["echo", "new_tool"]);
    s.ok(
        "PUT",
        &format!("/mcp-servers/{mid}"),
        Some(json!({ "name": "renamed" })),
    )
    .await;
    let t = s
        .ok("POST", &format!("/mcp-servers/{mid}/test"), None)
        .await;
    assert_eq!(t["tools"].as_array().unwrap().len(), 2);
    assert_eq!(frozen().await, before, "frozen_tools byte-identical");

    // The running session keeps its connection (old catalog). After a restart of its loop it
    // is served by the new connection: echo works, the vanished tool answers with an error.
    s.manager.cancel(&id).await.unwrap();
    s.wait_for(&id, |d| d["active"] == false).await;
    s.steer(&id, "use remote__echo and remote__fail please")
        .await;
    let d = s
        .wait_for(&id, |d| {
            status(d) == "idle" && d["raw"].as_array().unwrap().len() >= 4
        })
        .await;
    assert_eq!(tool_names(&d), ["remote__echo", "remote__fail"]);
    let results: Vec<&Value> = d["raw"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|m| m["role"] == "tool")
        .flat_map(|m| m["content"].as_array().unwrap())
        .collect();
    assert_eq!(results.len(), 2, "{results:?}");
    assert_eq!(results[0]["is_error"], false);
    assert!(results[0]["content"].as_str().unwrap().starts_with("echo:"));
    assert_eq!(results[1]["is_error"], true);
    assert!(
        results[1]["content"]
            .as_str()
            .unwrap()
            .contains("no longer available")
    );
    // The tools part of every request is the same.
    let turns = d["usage"]["per_turn"].as_array().unwrap();
    assert!(turns.len() >= 2);
    assert!(
        turns
            .iter()
            .all(|t| t["request"]["hash"]["tools"] == turns[0]["request"]["hash"]["tools"])
    );

    // New sessions see the new catalog under the new name.
    let id2 = s
        .create_with(
            json!({ "provider_id": "mock", "mcp_server_ids": [mid], "builtin_tools": false }),
        )
        .await;
    assert_eq!(
        tool_names(&s.get(&format!("/sessions/{id2}")).await),
        ["renamed__echo", "renamed__new_tool"]
    );

    // Deleting the server entirely: the old session still loads and resumes.
    s.manager.cancel(&id).await.unwrap();
    s.wait_for(&id, |d| d["active"] == false).await;
    s.ok("DELETE", &format!("/mcp-servers/{mid}"), None).await;
    assert_eq!(frozen().await, before);
    let n = d["raw"].as_array().unwrap().len();
    s.steer(&id, "use remote__echo").await;
    let d = s
        .wait_for(&id, |d| {
            status(d) == "idle" && d["raw"].as_array().unwrap().len() > n + 1
        })
        .await;
    let last_tool = d["raw"]
        .as_array()
        .unwrap()
        .iter()
        .rev()
        .find(|m| m["role"] == "tool")
        .unwrap();
    assert_eq!(last_tool["content"][0]["is_error"], true);
    s.stop().await;
}

// ----- presets -------------------------------------------------------------------------------

#[tokio::test]
async fn presets_crud_and_start_session() {
    let store = Arc::new(SqliteSessionStore::in_memory().await.unwrap());
    let s = Server::start(store, &workspace()).await;
    let mock = McpMock::default();
    mock.set_tools(&["echo"]);
    let url = mock.start().await;
    let mid = add_mcp(&s, "remote", &url).await;

    let p = s
        .ok(
            "POST",
            "/presets",
            Some(json!({
                "name": "Echo agent", "description": "d", "provider_id": "mock",
                "system_prompt": "be terse", "mcp_server_ids": [mid],
                "builtin_tools": false, "approval_policy": "always_ask"
            })),
        )
        .await;
    let pid = p["id"].as_str().unwrap().to_string();
    assert_eq!(p["model"], "");
    assert_eq!(s.get("/presets").await.as_array().unwrap().len(), 1);

    // Referenced records cannot be deleted while the preset exists.
    assert_eq!(
        s.send("DELETE", &format!("/mcp-servers/{mid}"), None)
            .await
            .status(),
        409
    );
    assert_eq!(
        s.send("DELETE", "/providers/mock", None).await.status(),
        409
    );
    // Unknown references are rejected.
    assert_eq!(
        s.send(
            "PUT",
            &format!("/presets/{pid}"),
            Some(json!({ "mcp_server_ids": ["nope"] }))
        )
        .await
        .status(),
        400
    );

    // One click: a session from the preset (model falls back to the provider default).
    let id = s
        .create_with(json!({ "preset_id": pid, "message": "call remote__echo" }))
        .await;
    let d = s.wait_for(&id, |d| status(d) == "needs_approval").await;
    assert_eq!(d["title"], "Echo agent");
    assert_eq!(d["system_prompt"], "be terse");
    assert_eq!(d["settings"]["model"], "mock");
    assert_eq!(d["approval_policy"], "always_ask");
    assert_eq!(tool_names(&d), ["remote__echo"]);

    // Explicit fields override the preset.
    let id2 = s
        .create_with(json!({ "preset_id": pid, "builtin_tools": true, "approval_policy": "auto_all", "title": "t2" }))
        .await;
    let d = s.get(&format!("/sessions/{id2}")).await;
    assert_eq!(tool_names(&d).len(), 5);
    assert_eq!(d["approval_policy"], "auto_all");
    assert_eq!(d["title"], "t2");

    let p = s
        .ok(
            "PUT",
            &format!("/presets/{pid}"),
            Some(json!({ "name": "Renamed" })),
        )
        .await;
    assert_eq!(p["name"], "Renamed");
    assert_eq!(p["system_prompt"], "be terse");
    s.ok("DELETE", &format!("/presets/{pid}"), None).await;
    assert!(s.get("/presets").await.as_array().unwrap().is_empty());
    s.stop().await;
}
