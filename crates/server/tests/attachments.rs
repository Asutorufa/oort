//! Image attachments end to end, against an in-process OpenAI-compatible provider.

mod common;

use std::sync::{Arc, Mutex};

use axum::extract::{Json, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use common::*;
use oort_storage::SqliteSessionStore;
use oort_storage::attachments::AttachmentStore;
use reqwest::multipart::{Form, Part};
use serde_json::{Value, json};

type Seen = Arc<Mutex<Vec<Value>>>;

/// Answers every turn; the model `blind` rejects images with a 400.
async fn completions(
    State(seen): State<Seen>,
    Json(body): Json<Value>,
) -> axum::response::Response {
    let title = body["messages"][0]["content"]
        .as_str()
        .is_some_and(|s| s.starts_with("You name chat conversations"));
    if !title {
        seen.lock().unwrap().push(body.clone());
    }
    if body["model"] == "blind" {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": { "message": "unsupported content type" } })),
        )
            .into_response();
    }
    let chunk =
        json!({ "choices": [{ "delta": { "content": "Seen." }, "finish_reason": "stop" }] });
    (
        [("content-type", "text/event-stream")],
        format!("data: {chunk}\n\ndata: [DONE]\n\n"),
    )
        .into_response()
}

/// A tiny valid-enough PNG header (`width` x `height`) followed by `tag`.
fn png(width: u32, height: u32, tag: u8) -> Vec<u8> {
    let mut out = b"\x89PNG\r\n\x1a\n\0\0\0\x0dIHDR".to_vec();
    out.extend(width.to_be_bytes());
    out.extend(height.to_be_bytes());
    out.extend([8, 6, 0, 0, 0, 0, 0, 0, 0, tag]);
    out
}

struct Setup {
    server: Server,
    store: Arc<SqliteSessionStore>,
    seen: Seen,
    provider: String,
}

impl Setup {
    async fn new(tag: &str) -> Self {
        let seen: Seen = Arc::default();
        let app = axum::Router::new()
            .route("/chat/completions", axum::routing::post(completions))
            .with_state(seen.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let data = temp_dir(tag);
        let db = format!("sqlite://{}/oort.db", data.display());
        let store = Arc::new(SqliteSessionStore::connect(&db).await.unwrap());
        assert_eq!(store.attachments_dir(), data.join("attachments"));
        let server = Server::start(store.clone(), &data, vec![]).await;
        let provider = server
            .ok(
                "POST",
                "/providers",
                Some(
                    json!({ "name": "fake", "kind": "openai_compatible", "base_url": url,
                             "default_model": "m" }),
                ),
            )
            .await["id"]
            .as_str()
            .unwrap()
            .to_string();
        Self {
            server,
            store,
            seen,
            provider,
        }
    }

    async fn upload(&self, files: &[Vec<u8>]) -> (u16, Value) {
        let mut form = Form::new();
        for (i, bytes) in files.iter().enumerate() {
            form = form.part(
                "files",
                Part::bytes(bytes.clone()).file_name(format!("dir/shot{i}.png")),
            );
        }
        let r = reqwest::Client::new()
            .post(format!("{}/attachments", self.server.base))
            .multipart(form)
            .send()
            .await
            .unwrap();
        (r.status().as_u16(), r.json().await.unwrap_or(Value::Null))
    }

    async fn ids(&self, files: &[Vec<u8>]) -> Vec<String> {
        let (status, body) = self.upload(files).await;
        assert_eq!(status, 200, "{body}");
        body["attachments"]
            .as_array()
            .unwrap()
            .iter()
            .map(|a| a["id"].as_str().unwrap().to_string())
            .collect()
    }

    async fn send_input(&self, session: &str, body: Value) -> u16 {
        self.server
            .send("POST", &format!("/sessions/{session}/input"), Some(body))
            .await
            .status()
            .as_u16()
    }

    async fn idle(&self, session: &str, messages: usize) -> Value {
        self.server
            .wait_for(session, |d| raw_len(d) >= messages && d["status"] == "idle")
            .await
    }

    fn files(&self) -> usize {
        std::fs::read_dir(self.store.attachments_dir()).map_or(0, |d| d.count())
    }
}

#[tokio::test]
async fn upload_validates_deduplicates_and_serves() {
    let t = Setup::new("att-upload").await;
    let a = png(3, 2, 1);
    let (status, body) = t.upload(&[a.clone(), png(5, 5, 2), a.clone()]).await;
    assert_eq!(status, 200, "{body}");
    let list = body["attachments"].as_array().unwrap();
    assert_eq!(list.len(), 3);
    assert_eq!(t.files(), 2, "identical content is stored once");
    assert_eq!(list[0]["mime_type"], "image/png");
    assert_eq!(list[0]["filename"], "shot0.png");
    assert_eq!(
        (list[0]["width"].as_u64(), list[0]["height"].as_u64()),
        (Some(3), Some(2))
    );
    assert_ne!(list[0]["id"], list[2]["id"]);

    let r = reqwest::get(format!(
        "{}{}",
        t.server.origin,
        list[0]["url"].as_str().unwrap()
    ))
    .await
    .unwrap();
    assert_eq!(r.headers()["content-type"], "image/png");
    assert_eq!(
        r.headers()["cache-control"],
        "private, max-age=31536000, immutable"
    );
    assert_eq!(r.bytes().await.unwrap().as_ref(), a);
    let missing = t.server.send("GET", "/attachments/att_nope", None).await;
    assert_eq!(missing.status(), 404);

    // One bad file rejects the whole request, whatever its name claims.
    let (status, body) = t.upload(&[png(1, 1, 9), b"<svg/>".to_vec()]).await;
    assert_eq!(status, 400, "{body}");
    assert_eq!(t.files(), 2);
    assert_eq!(t.upload(&vec![png(1, 1, 3); 7]).await.0, 400);
    assert_eq!(t.upload(&[]).await.0, 400);
    let mut big = png(1, 1, 4);
    big.resize(10 * 1024 * 1024 + 1, 0);
    let (status, body) = t.upload(&[big]).await;
    assert_eq!(status, 400, "{body}");

    assert_eq!(t.store.sweep_unsent_attachments(60_000).await.unwrap(), 0);
    assert_eq!(t.store.sweep_unsent_attachments(-60_000).await.unwrap(), 3);
    assert_eq!(t.files(), 0);
    t.server.stop().await;
}

#[tokio::test]
async fn images_are_referenced_in_history_and_inlined_on_the_wire() {
    let t = Setup::new("att-send").await;
    // An image alone can start a session.
    let first = t.ids(&[png(4, 4, 1), png(8, 8, 2)]).await;
    let id = t
        .server
        .create(json!({ "provider_id": t.provider, "attachment_ids": first, "widgets": false }))
        .await;
    let detail = t.idle(&id, 2).await;
    let content = detail["raw"][0]["content"].as_array().unwrap().clone();
    assert_eq!(content.len(), 2, "no empty text block: {content:?}");
    assert_eq!(content[0]["source"]["type"], "attachment");
    assert_eq!(content[0]["source"]["attachment_id"], first[0]);

    let second = t.ids(&[png(2, 2, 3)]).await;
    assert_eq!(
        t.send_input(
            &id,
            json!({ "type": "steer", "text": "and this?", "attachment_ids": second })
        )
        .await,
        202
    );
    t.idle(&id, 4).await;

    let stored: Vec<String> = sqlx::query_scalar("SELECT content_json FROM messages")
        .fetch_all(t.store.pool())
        .await
        .unwrap();
    assert!(
        stored
            .iter()
            .all(|c| !c.contains("base64") && !c.contains("iVBOR")),
        "{stored:?}"
    );

    let requests = t.seen.lock().unwrap().clone();
    let one = requests[0]["messages"].as_array().unwrap();
    let two = requests[1]["messages"].as_array().unwrap();
    let url = one.last().unwrap()["content"][1]["image_url"]["url"]
        .as_str()
        .unwrap();
    assert!(url.starts_with("data:image/png;base64,iVBOR"), "{url}");
    assert_eq!(
        &two[..one.len()],
        &one[..],
        "earlier turns are byte-identical"
    );

    // Limits and ownership.
    let seven = vec!["att_x"; 7];
    for bad in [
        json!({ "type": "steer", "text": "", "attachment_ids": [] }),
        json!({ "type": "steer", "text": "x", "attachment_ids": seven }),
        json!({ "type": "steer", "text": "x", "attachment_ids": ["att_unknown"] }),
        json!({ "type": "steer", "text": "/compact", "attachment_ids": second }),
    ] {
        assert_eq!(t.send_input(&id, bad.clone()).await, 400, "{bad}");
    }
    let other = t
        .server
        .create(json!({ "provider_id": t.provider, "widgets": false }))
        .await;
    assert_eq!(
        t.send_input(&other, json!({ "type": "steer", "attachment_ids": second }))
            .await,
        400,
        "another session's image"
    );
    let r = t
        .server
        .send(
            "POST",
            "/sessions",
            Some(json!({ "provider_id": t.provider, "attachment_ids": second })),
        )
        .await;
    assert_eq!(r.status(), 400);
    t.server.stop().await;
}

#[tokio::test]
async fn deleting_a_session_keeps_files_other_sessions_use() {
    let t = Setup::new("att-delete").await;
    let shared = png(4, 4, 1);
    let a = t.ids(&[shared.clone(), png(4, 4, 2)]).await;
    let b = t.ids(&[shared]).await;
    let mut sessions = Vec::new();
    for ids in [&a, &b] {
        let id = t
            .server
            .create(json!({ "provider_id": t.provider, "attachment_ids": ids, "widgets": false }))
            .await;
        t.idle(&id, 2).await;
        sessions.push(id);
    }
    assert_eq!(t.files(), 2);
    t.server
        .ok("DELETE", &format!("/sessions/{}", sessions[0]), None)
        .await;
    assert_eq!(t.files(), 1);
    let kept = t
        .server
        .send("GET", &format!("/attachments/{}", b[0]), None)
        .await;
    assert_eq!(kept.status(), 200);
    t.server
        .ok("DELETE", &format!("/sessions/{}", sessions[1]), None)
        .await;
    assert_eq!(t.files(), 0);
    t.server.stop().await;
}

#[tokio::test]
async fn a_rejected_image_request_says_why_it_may_have_failed() {
    let t = Setup::new("att-blind").await;
    let ids = t.ids(&[png(4, 4, 1)]).await;
    let id = t
        .server
        .create(
            json!({ "provider_id": t.provider, "model": "blind", "attachment_ids": ids,
                        "widgets": false }),
        )
        .await;
    let d = t.server.wait_for(&id, |d| !d["last_error"].is_null()).await;
    let message = d["last_error"]["message"].as_str().unwrap();
    assert!(message.contains("may not support image input"), "{message}");
    t.server.stop().await;
}
