//! A loop that fails (rate limit, network, …) reports why and waits for an explicit retry.

mod common;

use std::sync::Arc;

use common::*;
use oort_storage::SqliteSessionStore;
use serde_json::{Value, json};

async fn server(tag: &str) -> Server {
    let store = Arc::new(SqliteSessionStore::in_memory().await.unwrap());
    Server::start(store, &temp_dir(tag), vec![]).await
}

fn assistant_texts(d: &Value) -> usize {
    d["raw"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|m| m["role"] == "assistant")
        .count()
}

#[tokio::test]
async fn loop_errors_are_reported_and_retried_on_request() {
    let s = server("loop-err-retry").await;
    let id = s.create(json!({ "provider_id": "mock" })).await;
    s.steer(
        &id,
        "!error 429 once RESOURCE_EXHAUSTED: quota exceeded. Please retry in 13.1s.",
    )
    .await;

    let d = s.wait_for(&id, |d| !d["last_error"].is_null()).await;
    let msg = d["last_error"]["message"].as_str().unwrap();
    assert!(
        msg.contains("429") && msg.contains("RESOURCE_EXHAUSTED"),
        "{msg}"
    );
    assert!(d["last_error"]["at"].as_i64().unwrap() > 0);
    assert_eq!(d["active"], false);
    assert_eq!(assistant_texts(&d), 0);

    // Opening the event stream does not silently retry a failed loop.
    let r = s.send("GET", &format!("/sessions/{id}/events"), None).await;
    assert!(r.status().is_success());
    drop(r);
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    let d = s.get(&format!("/sessions/{id}")).await;
    assert!(!d["last_error"].is_null());
    assert_eq!(assistant_texts(&d), 0);

    // An explicit retry clears the error and answers.
    s.ok("POST", &format!("/sessions/{id}/retry"), None).await;
    let d = s
        .wait_for(&id, |d| assistant_texts(d) > 0 && d["status"] == "idle")
        .await;
    assert!(d["last_error"].is_null());
    s.stop().await;
}

#[tokio::test]
async fn new_input_after_an_error_clears_it() {
    let s = server("loop-err-input").await;
    let id = s.create(json!({ "provider_id": "mock" })).await;
    s.steer(&id, "!error 503 upstream unavailable").await;
    s.wait_for(&id, |d| !d["last_error"].is_null()).await;
    s.steer(&id, "hello again").await;
    // The new message joins the history; the failing line is no longer the last user message.
    let d = s
        .wait_for(&id, |d| d["last_error"].is_null() && assistant_texts(d) > 0)
        .await;
    assert_eq!(d["status"], "idle");
    s.stop().await;
}

#[tokio::test]
async fn auto_retry_is_off_by_default_and_follows_the_provider_setting() {
    let s = server("loop-err-auto").await;
    let p = s.get("/providers").await;
    let mock = p
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["kind"] == "mock")
        .unwrap();
    assert_eq!(
        mock["auto_retry"],
        json!({ "enabled": false, "max_attempts": 3 })
    );
    let pid = mock["id"].as_str().unwrap().to_string();

    // Off: the failure waits for the user.
    let id = s.create(json!({ "provider_id": pid })).await;
    s.steer(&id, "!error 429 once quota exceeded. Please retry in 0.1s.")
        .await;
    let d = s.wait_for(&id, |d| !d["last_error"].is_null()).await;
    assert!(d["last_error"]["retry_at"].is_null());

    // On: retried after the provider's hint (+1s), then answers.
    s.ok(
        "PUT",
        &format!("/providers/{pid}"),
        Some(json!({ "auto_retry": { "enabled": true, "max_attempts": 2 } })),
    )
    .await;
    let id = s.create(json!({ "provider_id": pid })).await;
    s.steer(
        &id,
        "!error 429 once quota exceeded (2). Please retry in 0.1s.",
    )
    .await;
    let d = s.wait_for(&id, |d| !d["last_error"].is_null()).await;
    let e = &d["last_error"];
    assert_eq!(e["attempt"], 0);
    let wait = e["retry_at"].as_i64().unwrap() - e["at"].as_i64().unwrap();
    assert!((1000..1500).contains(&wait), "{wait}");
    let d = s
        .wait_for(&id, |d| {
            d["last_error"].is_null() && assistant_texts(d) > 0 && d["status"] == "idle"
        })
        .await;
    assert_eq!(assistant_texts(&d), 1);

    // A persistent failure gives up after max_attempts.
    let id = s.create(json!({ "provider_id": pid })).await;
    s.steer(&id, "!error 503 down. Please retry in 0.05s.")
        .await;
    let d = s
        .wait_for(&id, |d| {
            d["last_error"]["attempt"] == 2 && d["last_error"]["retry_at"].is_null()
        })
        .await;
    assert_eq!(d["last_error"]["max_attempts"], 2);

    // Non-transient errors are never retried.
    let id = s.create(json!({ "provider_id": pid })).await;
    s.steer(&id, "!error 401 invalid api key").await;
    let d = s.wait_for(&id, |d| !d["last_error"].is_null()).await;
    assert!(d["last_error"]["retry_at"].is_null());
    s.stop().await;
}
