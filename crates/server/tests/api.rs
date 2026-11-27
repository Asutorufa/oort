//! End-to-end API tests: real HTTP server, in-memory SQLite, mock provider, built-in tools.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use futures_util::StreamExt;
use oort_server::{AgentFactory, SessionManager, router};
use oort_storage::SqliteSessionStore;
use serde_json::{Value, json};

struct Server {
    base: String,
    manager: Arc<SessionManager>,
    task: tokio::task::JoinHandle<()>,
}

impl Server {
    async fn start(store: Arc<SqliteSessionStore>, workspace: &std::path::Path) -> Self {
        let exec = Arc::new(oort_server::tools::builtin_executor(workspace));
        let factory = Arc::new(
            AgentFactory::with_executor(exec)
                .unwrap()
                .with_mock_word_delay(Duration::from_millis(5)),
        );
        let manager = SessionManager::new(store, factory);
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

    async fn post(&self, path: &str, body: Value) -> reqwest::Response {
        reqwest::Client::new()
            .post(format!("{}{path}", self.base))
            .json(&body)
            .send()
            .await
            .unwrap()
    }

    async fn get(&self, path: &str) -> Value {
        let r = reqwest::get(format!("{}{path}", self.base)).await.unwrap();
        assert!(r.status().is_success(), "GET {path}: {}", r.status());
        r.json().await.unwrap()
    }

    async fn create(&self, message: &str) -> String {
        let r = self
            .post(
                "/sessions",
                json!({ "provider": "mock", "model": "mock", "system_prompt": "You are a test agent.", "message": message }),
            )
            .await;
        assert_eq!(r.status(), 200);
        r.json::<Value>().await.unwrap()["id"]
            .as_str()
            .unwrap()
            .to_string()
    }

    /// Polls the snapshot until `pred` holds.
    async fn wait_for(&self, id: &str, pred: impl Fn(&Value) -> bool) -> Value {
        for _ in 0..200 {
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

    // Second turn via Steer: the cache-hit rate reflects the reused prefix.
    assert_eq!(
        s.post(
            &format!("/sessions/{id}/input"),
            json!({ "type": "steer", "text": "thanks" })
        )
        .await
        .status(),
        202
    );
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
    s.post(
        &format!("/sessions/{id}/input"),
        json!({ "type": "steer", "text": "tell me a slow story" }),
    )
    .await;
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
    assert!(buf.contains("data: {\"kind\":\"agent\""), "{buf}");
    assert!(buf.contains("id: 1"), "SSE ids carry the sequence number");
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
    assert_eq!(
        s.post(
            &format!("/sessions/{id}/input"),
            json!({ "type": "steer", "text": "back" })
        )
        .await
        .status(),
        202
    );
    s.wait_for(&id, |d| d["raw"].as_array().unwrap().len() == 5)
        .await;
    s.stop().await;
}

#[tokio::test]
async fn errors_are_json() {
    let store = Arc::new(SqliteSessionStore::in_memory().await.unwrap());
    let s = Server::start(store, &workspace()).await;
    let r = reqwest::get(format!("{}/sessions/missing", s.base))
        .await
        .unwrap();
    assert_eq!(r.status(), 404);
    assert!(
        r.json::<Value>().await.unwrap()["error"]
            .as_str()
            .unwrap()
            .contains("missing")
    );
    let r = s
        .post("/sessions", json!({ "provider": "mock", "model": "" }))
        .await;
    assert_eq!(r.status(), 400);
    s.stop().await;
}

#[tokio::test]
async fn shutdown_ends_open_sse_streams() {
    let store = Arc::new(SqliteSessionStore::in_memory().await.unwrap());
    let s = Server::start(store, &workspace()).await;
    let id = s.create("hi").await;
    let resp = reqwest::get(format!("{}/sessions/{id}/events", s.base))
        .await
        .unwrap();
    let mut body = resp.bytes_stream();
    s.manager.shutdown().await;
    // Without this, axum's graceful shutdown would wait on the stream forever.
    let end = tokio::time::timeout(Duration::from_secs(5), async {
        while let Some(chunk) = body.next().await {
            chunk.unwrap();
        }
    })
    .await;
    assert!(end.is_ok(), "SSE stream stayed open after shutdown");
    s.task.abort();
}
