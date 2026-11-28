//! Provider generation defaults, the thinking level (provider → preset → session, changeable
//! mid-conversation) and model listing.

mod common;

use std::sync::Arc;

use common::*;
use oort_storage::SqliteSessionStore;
use serde_json::{Value, json};

async fn server(tag: &str) -> Server {
    let store = Arc::new(SqliteSessionStore::in_memory().await.unwrap());
    Server::start(store, &temp_dir(tag), vec![]).await
}

async fn mock_id(s: &Server) -> String {
    let p = s.get("/providers").await;
    p.as_array()
        .unwrap()
        .iter()
        .find(|p| p["kind"] == "mock")
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string()
}

/// The parameters of the latest request, as echoed by the mock model.
async fn params(s: &Server, id: &str) -> Value {
    let before = s.get(&format!("/sessions/{id}")).await["raw"]
        .as_array()
        .unwrap()
        .len();
    s.steer(id, "!params").await;
    let d = s
        .wait_for(id, |d| {
            d["raw"].as_array().unwrap().len() >= before + 2 && d["status"] == "idle"
        })
        .await;
    let last = d["raw"].as_array().unwrap().last().unwrap().clone();
    let text = last["content"][0]["text"].as_str().unwrap().to_string();
    serde_json::from_str(&text).unwrap()
}

#[tokio::test]
async fn provider_defaults_round_trip_and_are_validated() {
    let s = server("prov-defaults").await;
    let pid = mock_id(&s).await;
    let p = s.get(&format!("/providers/{pid}")).await;
    assert_eq!(
        p["defaults"],
        json!({ "reasoning_effort": "default", "temperature": null, "max_output_tokens": null,
                "context_window": null, "timeout_secs": 120 })
    );
    let defaults = json!({ "reasoning_effort": "high", "temperature": 0.25, "max_output_tokens": 2048,
                           "context_window": 64000, "timeout_secs": 60 });
    s.ok(
        "PUT",
        &format!("/providers/{pid}"),
        Some(json!({ "defaults": defaults })),
    )
    .await;
    assert_eq!(
        s.get(&format!("/providers/{pid}")).await["defaults"],
        defaults
    );

    for bad in [
        json!({ "reasoning_effort": "high", "temperature": 3.0, "max_output_tokens": null, "context_window": null, "timeout_secs": 60 }),
        json!({ "reasoning_effort": "high", "temperature": null, "max_output_tokens": null, "context_window": null, "timeout_secs": 1 }),
        json!({ "reasoning_effort": "extreme", "temperature": null, "max_output_tokens": null, "context_window": null, "timeout_secs": 60 }),
    ] {
        let r = s
            .send(
                "PUT",
                &format!("/providers/{pid}"),
                Some(json!({ "defaults": bad })),
            )
            .await;
        assert!(r.status().is_client_error(), "{bad}");
    }
    s.stop().await;
}

#[tokio::test]
async fn thinking_level_flows_from_provider_to_preset_to_session_and_changes_mid_chat() {
    let s = server("prov-thinking").await;
    let pid = mock_id(&s).await;
    s.ok(
        "PUT",
        &format!("/providers/{pid}"),
        Some(json!({ "defaults": { "reasoning_effort": "low", "temperature": 0.5, "max_output_tokens": 1234,
                                   "context_window": 50000, "timeout_secs": 120 } })),
    )
    .await;

    // Provider defaults apply to a plain session.
    let id = s.create(json!({ "provider_id": pid })).await;
    let d = s.get(&format!("/sessions/{id}")).await;
    assert_eq!(d["reasoning_effort"], "default");
    assert_eq!(d["effective_reasoning_effort"], "low");
    assert_eq!(d["settings"]["max_context_tokens"], 50000);
    let p = params(&s, &id).await;
    assert_eq!(p["thinking"], json!({ "enabled": true, "effort": "low" }));
    assert_eq!(p["max_tokens"], 1234);
    assert!((p["temperature"].as_f64().unwrap() - 0.5).abs() < 1e-6);

    // Changing it mid-conversation affects the next request, not the history.
    let history = s.get(&format!("/sessions/{id}")).await["raw"].clone();
    s.ok(
        "PATCH",
        &format!("/sessions/{id}"),
        Some(json!({ "reasoning_effort": "off" })),
    )
    .await;
    let d = s.get(&format!("/sessions/{id}")).await;
    assert_eq!(
        (
            d["reasoning_effort"].as_str(),
            d["effective_reasoning_effort"].as_str()
        ),
        (Some("off"), Some("off"))
    );
    assert_eq!(
        params(&s, &id).await["thinking"],
        json!({ "enabled": false })
    );
    let raw = s.get(&format!("/sessions/{id}")).await["raw"].clone();
    assert_eq!(
        raw.as_array().unwrap()[..history.as_array().unwrap().len()],
        history.as_array().unwrap()[..]
    );

    // Back to the provider's default.
    s.ok(
        "PATCH",
        &format!("/sessions/{id}"),
        Some(json!({ "reasoning_effort": "default" })),
    )
    .await;
    assert_eq!(
        params(&s, &id).await["thinking"],
        json!({ "enabled": true, "effort": "low" })
    );

    // Presets and explicit requests override the provider.
    let preset = s
        .ok(
            "POST",
            "/presets",
            Some(json!({ "name": "deep", "provider_id": pid, "reasoning_effort": "high" })),
        )
        .await;
    assert_eq!(preset["reasoning_effort"], "high");
    let id = s.create(json!({ "preset_id": preset["id"] })).await;
    assert_eq!(
        s.get(&format!("/sessions/{id}")).await["reasoning_effort"],
        "high"
    );
    assert_eq!(
        params(&s, &id).await["thinking"],
        json!({ "enabled": true, "effort": "high" })
    );
    let id = s
        .create(json!({ "preset_id": preset["id"], "reasoning_effort": "medium" }))
        .await;
    assert_eq!(
        s.get(&format!("/sessions/{id}")).await["reasoning_effort"],
        "medium"
    );
    s.stop().await;
}

#[tokio::test]
async fn model_lists_come_from_the_provider_or_explain_why_not() {
    let s = server("prov-models").await;
    let pid = mock_id(&s).await;
    let m = s
        .ok("POST", &format!("/providers/{pid}/models"), None)
        .await;
    assert_eq!(m["models"], json!(["mock"]));
    let m = s
        .ok("POST", "/providers/models", Some(json!({ "kind": "mock" })))
        .await;
    assert_eq!(m["models"], json!(["mock"]));

    // An unreachable server is a gateway error with a message (the UI falls back to typing).
    let r = s
        .send(
            "POST",
            "/providers/models",
            Some(json!({ "kind": "openai_compatible", "base_url": "http://127.0.0.1:9/v1",
                         "defaults": { "reasoning_effort": "default", "temperature": null, "max_output_tokens": null,
                                       "context_window": null, "timeout_secs": 5 } })),
        )
        .await;
    assert_eq!(r.status(), 502);
    assert!(r.text().await.unwrap().contains("list models"));
    // Missing pieces are reported as such.
    let r = s
        .send(
            "POST",
            "/providers/models",
            Some(json!({ "kind": "openai_compatible" })),
        )
        .await;
    assert_eq!(r.status(), 400);
    s.stop().await;
}

#[tokio::test]
async fn the_model_can_be_switched_mid_chat_without_touching_history() {
    let s = server("prov-model-switch").await;
    let pid = mock_id(&s).await;
    let id = s
        .create(json!({ "provider_id": pid, "model": "mock" }))
        .await;
    assert_eq!(params(&s, &id).await["model"], "mock");
    let history = s.get(&format!("/sessions/{id}")).await["raw"].clone();

    s.ok(
        "PATCH",
        &format!("/sessions/{id}"),
        Some(json!({ "model": "  mock-large  " })),
    )
    .await;
    let d = s.get(&format!("/sessions/{id}")).await;
    assert_eq!(d["model"], "mock-large");
    assert_eq!(d["settings"]["model"], "mock-large");
    assert_eq!(params(&s, &id).await["model"], "mock-large");
    let raw = s.get(&format!("/sessions/{id}")).await["raw"].clone();
    assert_eq!(
        raw.as_array().unwrap()[..history.as_array().unwrap().len()],
        history.as_array().unwrap()[..]
    );

    // The list shows it, and it survives a restart of the loop (stored, not only live).
    let list = s.get("/sessions").await;
    assert_eq!(
        list.as_array()
            .unwrap()
            .iter()
            .find(|x| x["id"] == id.as_str())
            .unwrap()["model"],
        "mock-large"
    );
    s.ok("POST", &format!("/sessions/{id}/cancel"), None).await;
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    assert_eq!(params(&s, &id).await["model"], "mock-large");

    for bad in [json!(""), json!("   "), json!("a\nb")] {
        let r = s
            .send(
                "PATCH",
                &format!("/sessions/{id}"),
                Some(json!({ "model": bad })),
            )
            .await;
        assert_eq!(r.status(), 400, "{bad}");
    }
    s.stop().await;
}
