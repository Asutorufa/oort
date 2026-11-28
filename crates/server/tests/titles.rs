//! Session titles end to end, against an in-process OpenAI-compatible provider.

mod common;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::extract::{Json, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use common::*;
use oort_agent::SessionStore;
use oort_storage::SqliteSessionStore;
use serde_json::{Value, json};
use tokio::sync::Semaphore;

/// What the fake provider does with title (sidecar) requests.
#[derive(Clone, Copy)]
enum Sidecar {
    Reply(&'static str),
    Fail,
}

#[derive(Clone)]
struct Fake {
    requests: Arc<Mutex<Vec<Value>>>,
    sidecar: Sidecar,
    /// Native title in conversation replies.
    native: Option<&'static str>,
    /// A sidecar reply waits for a permit.
    gate: Arc<Semaphore>,
}

fn is_sidecar(body: &Value) -> bool {
    body["messages"][0]["content"]
        .as_str()
        .is_some_and(|s| s.starts_with("You name chat conversations"))
}

async fn completions(
    State(fake): State<Fake>,
    Json(body): Json<Value>,
) -> axum::response::Response {
    fake.requests.lock().unwrap().push(body.clone());
    let mut delta = json!({ "content": "An answer." });
    if is_sidecar(&body) {
        fake.gate.acquire().await.unwrap().forget();
        match fake.sidecar {
            Sidecar::Reply(title) => delta = json!({ "content": title }),
            Sidecar::Fail => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
        }
    } else if let Some(title) = fake.native
        && body.get("title").is_some()
    {
        delta["title"] = json!(title);
    }
    let chunk = json!({ "choices": [{ "delta": delta, "finish_reason": "stop" }] });
    (
        [("content-type", "text/event-stream")],
        format!("data: {chunk}\n\ndata: [DONE]\n\n"),
    )
        .into_response()
}

struct Setup {
    server: Server,
    fake: Fake,
    provider: String,
}

impl Setup {
    async fn new(tag: &str, vendor: &str, sidecar: Sidecar, native: Option<&'static str>) -> Self {
        let fake = Fake {
            requests: Arc::default(),
            sidecar,
            native,
            gate: Arc::new(Semaphore::new(100)),
        };
        let app = axum::Router::new()
            .route("/chat/completions", axum::routing::post(completions))
            .with_state(fake.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let store = Arc::new(SqliteSessionStore::in_memory().await.unwrap());
        let server = Server::start(store, &temp_dir(tag), vec![]).await;
        let provider = server
            .ok(
                "POST",
                "/providers",
                Some(
                    json!({ "name": "fake", "kind": "openai_compatible", "vendor": vendor,
                             "base_url": url, "default_model": "m" }),
                ),
            )
            .await["id"]
            .as_str()
            .unwrap()
            .to_string();
        Self {
            server,
            fake,
            provider,
        }
    }

    async fn create(&self, message: &str) -> String {
        self.server
            .create(json!({ "provider_id": self.provider, "message": message, "widgets": false }))
            .await
    }

    async fn titled(&self, id: &str, source: &str) -> Value {
        self.server
            .wait_for(id, |d| d["title_source"] == source && d["status"] == "idle")
            .await
    }

    /// A second turn, waited for.
    async fn again(&self, id: &str) {
        let before = raw_len(&self.server.get(&format!("/sessions/{id}")).await);
        self.server.steer(id, "and then?").await;
        self.server
            .wait_for(id, |d| raw_len(d) >= before + 2 && d["status"] == "idle")
            .await;
        // Give a (wrong) second sidecar request the chance to show up.
        tokio::time::sleep(Duration::from_millis(200)).await;
    }

    fn requests(&self) -> (Vec<Value>, Vec<Value>) {
        let all = self.fake.requests.lock().unwrap().clone();
        let (side, main): (Vec<_>, Vec<_>) = all.into_iter().partition(is_sidecar);
        (main, side)
    }
}

#[tokio::test]
async fn deepseek_native_title_costs_no_extra_request() {
    let t = Setup::new(
        "title-native",
        "deepseek",
        Sidecar::Fail,
        Some("「Native」"),
    )
    .await;
    let id = t.create("question").await;
    assert_eq!(t.titled(&id, "auto").await["title"], "Native");
    t.again(&id).await;
    let (main, side) = t.requests();
    assert!(side.is_empty(), "{side:?}");
    assert_eq!(main[0]["title"], json!({ "enabled": true }));
    assert!(main[1].get("title").is_none(), "{}", main[1]);
    t.server.stop().await;
}

#[tokio::test]
async fn deepseek_without_a_title_falls_back_to_the_sidecar() {
    let t = Setup::new("title-ds-side", "deepseek", Sidecar::Reply("Side"), None).await;
    let id = t.create("question").await;
    assert_eq!(t.titled(&id, "auto").await["title"], "Side");
    let (_, side) = t.requests();
    assert_eq!(side.len(), 1);
    assert_eq!(side[0]["thinking"], json!({ "type": "disabled" }));
    assert!(side[0].get("title").is_none());
    t.server.stop().await;
}

#[tokio::test]
async fn sidecar_runs_once_apart_from_the_conversation() {
    let t = Setup::new(
        "title-side",
        "custom",
        Sidecar::Reply("**\"Borrowing\"**."),
        None,
    )
    .await;
    let long = format!("explain borrowing {}", "x".repeat(900));
    let id = t.create(&long).await;
    assert_eq!(t.titled(&id, "auto").await["title"], "Borrowing");
    t.again(&id).await;
    let (main, side) = t.requests();
    assert_eq!((main.len(), side.len()), (2, 1));
    assert!(main.iter().all(|r| r.get("title").is_none()));
    // The conversation only grows: nothing about titles enters its prefix.
    let first = main[0]["messages"].as_array().unwrap();
    assert_eq!(
        &main[1]["messages"].as_array().unwrap()[..first.len()],
        &first[..]
    );
    assert!(!main[1].to_string().contains("name chat conversations"));
    let side = &side[0];
    assert!(side.get("tools").is_none(), "{side}");
    assert!(side.get("reasoning_effort").is_none());
    assert_eq!(side["max_tokens"], 64);
    assert_eq!(side["messages"].as_array().unwrap().len(), 2);
    let input = side["messages"][1]["content"].as_str().unwrap();
    assert!(input.contains("An answer.") && input.len() < 700, "{input}");
    t.server.stop().await;
}

#[tokio::test]
async fn a_failed_sidecar_leaves_a_tidy_fallback() {
    let t = Setup::new("title-fallback", "custom", Sidecar::Fail, None).await;
    let id = t.create("fix\nthe build").await;
    assert_eq!(t.titled(&id, "auto").await["title"], "fix the build");
    t.server.stop().await;
}

#[tokio::test]
async fn a_late_generated_title_never_replaces_a_rename() {
    let t = Setup::new("title-race", "custom", Sidecar::Reply("Generated"), None).await;
    t.fake.gate.forget_permits(100);
    let id = t.create("question").await;
    t.server.wait_for(&id, |d| d["status"] == "idle").await;
    for _ in 0..100 {
        if !t.requests().1.is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(t.requests().1.len(), 1);
    t.server
        .ok(
            "PATCH",
            &format!("/sessions/{id}"),
            Some(json!({ "title": "Mine" })),
        )
        .await;
    t.fake.gate.add_permits(1);
    tokio::time::sleep(Duration::from_millis(300)).await;
    let d = t.server.get(&format!("/sessions/{id}")).await;
    assert_eq!(
        (&d["title"], &d["title_source"]),
        (&json!("Mine"), &json!("manual"))
    );

    // Asking for a new one is the user's own choice.
    t.fake.gate.add_permits(1);
    let d = t
        .server
        .ok("POST", &format!("/sessions/{id}/title/regenerate"), None)
        .await;
    assert_eq!(
        (&d["title"], &d["title_source"]),
        (&json!("Generated"), &json!("auto"))
    );
    t.server.stop().await;
}

#[tokio::test]
async fn subagent_sessions_are_never_named() {
    let t = Setup::new("title-child", "custom", Sidecar::Reply("Generated"), None).await;
    let id = t
        .server
        .create(
            json!({ "provider_id": "mock", "builtin_groups": ["subagent"], "widgets": false,
                        "message": "@spawn_subagent {\"task\": \"look around\"}" }),
        )
        .await;
    t.server
        .wait_for(&id, |d| d["title_source"] == "auto")
        .await;
    let children = t.server.store.list_children(&id).await.unwrap();
    assert_eq!(children.len(), 1);
    assert_eq!(children[0].title_source.as_str(), "default");
    assert!(
        !t.server
            .store
            .set_auto_title(&children[0].id, "x")
            .await
            .unwrap()
    );
    let r = t
        .server
        .send(
            "POST",
            &format!("/sessions/{}/title/regenerate", children[0].id),
            None,
        )
        .await;
    assert_eq!(r.status(), 409);
    t.server.stop().await;
}

#[tokio::test]
async fn a_failed_first_turn_gets_a_provisional_title() {
    let t = Setup::new("title-error", "custom", Sidecar::Fail, None).await;
    let id = t
        .server
        .create(json!({ "provider_id": "mock", "message": "!error 401 nope" }))
        .await;
    let d = t.server.wait_for(&id, |d| d["title"] != "").await;
    assert_eq!(d["title"], "!error 401 nope");
    assert_eq!(d["title_source"], "default");
    t.server.stop().await;
}
