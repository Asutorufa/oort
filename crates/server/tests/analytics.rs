//! Usage & budget end to end: model and tool calls reach the ledgers, deleting a chat keeps
//! them, and the budget / price endpoints validate and apply.

mod common;

use std::sync::Arc;

use common::{Server, status, temp_dir};
use oort_storage::SqliteSessionStore;
use serde_json::json;

#[tokio::test]
async fn usage_is_recorded_and_survives_deleting_the_chat() {
    let dir = temp_dir("analytics");
    let store = Arc::new(SqliteSessionStore::in_memory().await.unwrap());
    let server = Server::start(store, &dir, vec![]).await;
    let report = "/analytics/usage?range=7d&tz_offset_minutes=540";

    let empty = server.get(report).await;
    assert_eq!(empty["total_records"], 0);
    assert_eq!(empty["summary"]["cache_hit_rate_pct"], 0.0);
    assert_eq!(empty["budget"]["status"], "unset");

    let id = server
        .create(json!({
            "provider_id": "mock", "system_prompt": "test", "message": "what is the time",
            "title": "Clock", "widgets": false, "builtin_groups": ["utilities"],
            "approval_policy": "auto_all"
        }))
        .await;
    server.wait_for(&id, |d| status(d) == "idle").await;

    let used = server.get(report).await;
    let turns = used["summary"]["turns"].as_u64().unwrap();
    assert!(turns >= 1, "{used}");
    assert_eq!(used["total_records"].as_u64().unwrap(), turns);
    assert!(used["summary"]["total_tokens"].as_u64().unwrap() > 0);
    assert_eq!(used["by_model"][0]["model"], "mock");
    assert_eq!(used["by_model"][0]["has_pricing"], false);
    assert_eq!(used["top_sessions"][0]["session_id"], id.as_str());
    assert_eq!(used["top_sessions"][0]["title"], "Clock");

    // A custom price applies to what was already recorded.
    server
        .ok(
            "PUT",
            "/analytics/pricing",
            Some(json!({ "model_pattern": "mock", "input_per_m": 1000, "output_per_m": 2000 })),
        )
        .await;
    let priced = server.get(report).await;
    let cost = priced["summary"]["cost_usd"].as_f64().unwrap();
    assert!(cost > 0.0);
    assert_eq!(priced["by_model"][0]["has_pricing"], true);

    let budget = server
        .ok(
            "PUT",
            "/analytics/budget?tz_offset_minutes=540",
            Some(json!({ "monthly_budget_usd": cost / 2.0, "warning_threshold_pct": 80 })),
        )
        .await;
    assert_eq!(budget["status"], "exceeded");

    server.ok("DELETE", &format!("/sessions/{id}"), None).await;
    let kept = server.get(report).await;
    assert_eq!(kept["summary"]["turns"].as_u64().unwrap(), turns);
    assert!((kept["summary"]["cost_usd"].as_f64().unwrap() - cost).abs() < 1e-9);
    assert_eq!(kept["top_sessions"][0]["exists"], false);
    assert_eq!(kept["top_sessions"][0]["title"], "Clock");

    let left = server
        .ok("DELETE", "/analytics/pricing?model_pattern=mock", None)
        .await;
    assert_eq!(left, json!([]));

    for (method, path, body) in [
        ("GET", "/analytics/usage?range=1y&tz_offset_minutes=0", None),
        ("GET", "/analytics/usage?range=7d", None),
        (
            "GET",
            "/analytics/usage?tz_offset_minutes=0&calendar_month=2026-13",
            None,
        ),
        (
            "PUT",
            "/analytics/budget",
            Some(json!({ "monthly_budget_usd": -1, "warning_threshold_pct": 80 })),
        ),
        (
            "PUT",
            "/analytics/pricing",
            Some(json!({ "model_pattern": "*", "input_per_m": 1, "output_per_m": 1 })),
        ),
    ] {
        let r = server.send(method, path, body).await;
        assert_eq!(r.status(), 400, "{method} {path}");
    }
    server.stop().await;
}

#[tokio::test]
async fn tool_calls_are_recorded() {
    let dir = temp_dir("analytics-tools");
    let store = Arc::new(SqliteSessionStore::in_memory().await.unwrap());
    let server = Server::start(store, &dir, vec![]).await;
    let id = server
        .create(json!({
            "provider_id": "mock", "system_prompt": "test", "message": "what is the time",
            "widgets": false, "approval_policy": "auto_all"
        }))
        .await;
    let detail = server.wait_for(&id, |d| status(d) == "idle").await;
    let report = server
        .get("/analytics/usage?range=7d&tz_offset_minutes=0")
        .await;
    let tools = report["top_tools"].as_array().unwrap();
    assert_eq!(tools.len(), 1, "{report}\n{detail}");
    assert_eq!(tools[0]["tool_kind"], "builtin");
    assert_eq!(tools[0]["calls"], 1);
    server.stop().await;
}
