//! Skills (progressive disclosure) and Intelligent UI widgets, end to end.

mod common;

use std::sync::Arc;

use common::*;
use oort_storage::SqliteSessionStore;
use serde_json::{Value, json};

fn write_skill(dir: &std::path::Path, name: &str, text: &str) {
    std::fs::create_dir_all(dir.join(name)).unwrap();
    std::fs::write(dir.join(name).join("SKILL.md"), text).unwrap();
}

#[tokio::test]
async fn skills_crud_reload_and_file_skills_are_read_only() {
    let skills_dir = temp_dir("skills-a");
    write_skill(
        &skills_dir,
        "code-review",
        "---\nname: code-review\ndescription: Review a diff carefully\nallowed_tools: [read_file]\ntags: [dev]\n---\n# Steps\n1. Read every hunk.",
    );
    write_skill(&skills_dir, "broken", "no frontmatter here");
    let store = Arc::new(SqliteSessionStore::in_memory().await.unwrap());
    let s = Server::start(store, &temp_dir("ws-a"), vec![skills_dir.clone()]).await;

    let list = s.get("/skills").await;
    assert_eq!(list.as_array().unwrap().len(), 1);
    assert_eq!(list[0]["id"], "file:code-review");
    assert_eq!(list[0]["editable"], false);
    assert_eq!(list[0]["source"]["kind"], "file");
    assert_eq!(list[0]["allowed_tools"], json!(["read_file"]));

    // File skills: edit the file instead.
    let r = s
        .send(
            "PUT",
            "/skills/file:code-review",
            Some(json!({ "body": "x" })),
        )
        .await;
    assert_eq!(r.status(), 409);
    assert_eq!(
        s.send("DELETE", "/skills/file:code-review", None)
            .await
            .status(),
        409
    );

    // DB skills: create / validate / update / delete.
    let k = s
        .ok("POST", "/skills", Some(json!({ "name": "release-notes", "description": "Write release notes", "body": "Use bullet points." })))
        .await;
    let kid = k["id"].as_str().unwrap().to_string();
    assert_eq!(k["editable"], true);
    for bad in [
        json!({ "name": "Bad Name", "description": "d" }),
        json!({ "name": "x" }),
    ] {
        assert_eq!(s.send("POST", "/skills", Some(bad)).await.status(), 400);
    }
    assert_eq!(
        s.send(
            "POST",
            "/skills",
            Some(json!({ "name": "release-notes", "description": "dup" }))
        )
        .await
        .status(),
        409
    );
    let k = s
        .ok(
            "PUT",
            &format!("/skills/{kid}"),
            Some(json!({ "body": "Use numbered points." })),
        )
        .await;
    assert_eq!(k["body"], "Use numbered points.");
    assert_eq!(k["description"], "Write release notes");

    // Reload picks up new files and reports broken ones.
    write_skill(
        &skills_dir,
        "triage",
        "---\ndescription: Triage an incident\n---\nCheck dashboards.",
    );
    let r = s.ok("POST", "/skills/reload", None).await;
    assert_eq!(r["file_skills"], 2);
    assert_eq!(r["warnings"].as_array().unwrap().len(), 1, "{r}");
    assert_eq!(s.get("/skills").await.as_array().unwrap().len(), 3);

    s.ok("DELETE", &format!("/skills/{kid}"), None).await;
    assert_eq!(s.get("/skills").await.as_array().unwrap().len(), 2);
    s.stop().await;
}

#[tokio::test]
async fn skills_are_disclosed_progressively_without_touching_the_prefix() {
    let skills_dir = temp_dir("skills-b");
    write_skill(
        &skills_dir,
        "zeta-guide",
        "---\ndescription: Last alphabetically\n---\nZETA BODY",
    );
    write_skill(
        &skills_dir,
        "code-review",
        "---\ndescription: Review a diff\n---\nREVIEW BODY: read every hunk.",
    );
    let store = Arc::new(SqliteSessionStore::in_memory().await.unwrap());
    let s = Server::start(store, &temp_dir("ws-b"), vec![skills_dir]).await;
    let db = s
        .ok("POST", "/skills", Some(json!({ "name": "alpha-db", "description": "From the database", "body": "ALPHA BODY" })))
        .await;

    let preset = s
        .ok(
            "POST",
            "/presets",
            Some(json!({
                "name": "with skills", "provider_id": "mock", "system_prompt": "Base prompt.",
                "builtin_tools": false,
                "skill_ids": ["file:zeta-guide", db["id"], "file:code-review"]
            })),
        )
        .await;
    assert_eq!(
        s.send(
            "POST",
            "/presets",
            Some(json!({ "name": "bad", "provider_id": "mock", "skill_ids": ["file:nope"] }))
        )
        .await
        .status(),
        400
    );
    let id = s
        .create(json!({ "preset_id": preset["id"], "widgets": false }))
        .await;
    let d = s.get(&format!("/sessions/{id}")).await;

    // Frozen prefix: the index (names + descriptions, sorted) only — never a body.
    let sp = d["system_prompt"].as_str().unwrap();
    assert!(sp.starts_with("Base prompt.\n\n<available_skills>"), "{sp}");
    let (a, c, z) = (
        sp.find("- alpha-db:").unwrap(),
        sp.find("- code-review:").unwrap(),
        sp.find("- zeta-guide:").unwrap(),
    );
    assert!(a < c && c < z, "sorted by name");
    assert!(!sp.contains("BODY"));
    assert_eq!(tool_names(&d), ["load_skill"]);
    assert_eq!(
        d["tools"][0]["parameters"]["properties"]["name"]["enum"],
        json!(["alpha-db", "code-review", "zeta-guide"])
    );
    assert_eq!(d["settings"]["skills"].as_array().unwrap().len(), 3);

    // Turn 1: plain. Turn 2: load_skill → body arrives as a tool result.
    s.steer(&id, "hello").await;
    s.wait_for(&id, |d| status(d) == "idle" && raw_len(d) == 2)
        .await;
    s.steer(&id, "please use skill:code-review").await;
    let d = s
        .wait_for(&id, |d| status(d) == "idle" && raw_len(d) == 6)
        .await;
    let r = &results(&d)[0];
    assert_eq!(r["is_error"], false);
    assert!(
        r["content"].as_str().unwrap().contains("REVIEW BODY"),
        "{r}"
    );

    // Turn 3: `/name` appends the body at the tail as a user message.
    s.steer(&id, "/zeta-guide do it now").await;
    let d = s
        .wait_for(&id, |d| status(d) == "idle" && raw_len(d) == 8)
        .await;
    let user = d["raw"][6]["content"][0]["text"].as_str().unwrap();
    assert!(
        user.starts_with("<skill name=\"zeta-guide\">")
            && user.contains("ZETA BODY")
            && user.ends_with("do it now"),
        "{user}"
    );
    assert_eq!(
        s.send(
            "POST",
            &format!("/sessions/{id}/input"),
            Some(json!({ "type": "steer", "text": "/no-such-skill hi" }))
        )
        .await
        .status(),
        400
    );

    // The system prompt and tool catalog never changed: identical prefix hashes on every call.
    assert_eq!(d["system_prompt"], sp);
    let turns = d["usage"]["per_turn"].as_array().unwrap();
    assert!(turns.len() >= 4);
    for t in turns {
        assert_eq!(
            t["request"]["hash"]["system"],
            turns[0]["request"]["hash"]["system"]
        );
        assert_eq!(
            t["request"]["hash"]["tools"],
            turns[0]["request"]["hash"]["tools"]
        );
    }
    assert!(turns.last().unwrap()["cache_hit_rate"].as_f64().unwrap() > 0.5);

    // Editing a skill later changes what load_skill returns, never the frozen index.
    let db_id = db["id"].as_str().unwrap();
    s.ok(
        "PUT",
        &format!("/skills/{db_id}"),
        Some(json!({ "description": "CHANGED", "body": "NEW ALPHA" })),
    )
    .await;
    s.steer(&id, "skill:alpha-db").await;
    let d = s
        .wait_for(&id, |d| status(d) == "idle" && raw_len(d) == 12)
        .await;
    assert_eq!(d["system_prompt"], sp, "index frozen");
    assert!(
        results(&d).last().unwrap()["content"]
            .as_str()
            .unwrap()
            .contains("NEW ALPHA")
    );

    // `/compact` is a command, not a message.
    s.steer(&id, "/compact").await;
    s.wait_for(&id, |d| !d["checkpoints"].as_array().unwrap().is_empty())
        .await;
    s.stop().await;
}

fn widget_calls(d: &Value) -> Vec<Value> {
    d["raw"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|m| m["content"].as_array().unwrap().clone())
        .filter(|p| p["kind"] == "tool_call" && p["name"] == "render_widget")
        .collect()
}

#[tokio::test]
async fn widgets_render_persist_and_feed_back_through_the_tail() {
    let store = Arc::new(SqliteSessionStore::in_memory().await.unwrap());
    let ws = temp_dir("ws-c");
    let s = Server::start(store.clone(), &ws, vec![]).await;
    let id = s
        .create(json!({ "provider_id": "mock", "builtin_tools": false }))
        .await;
    assert_eq!(
        tool_names(&s.get(&format!("/sessions/{id}")).await),
        ["render_widget"]
    );

    let events = reqwest::get(format!("{}/sessions/{id}/events", s.base))
        .await
        .unwrap();
    s.steer(&id, "show widget:metrics_chart and widget:interactive_form")
        .await;
    let ev = read_sse(events, |ev| {
        ev.iter().filter(|e| e["kind"] == "widget").count() >= 2
    })
    .await;
    let w: Vec<&Value> = ev.iter().filter(|e| e["kind"] == "widget").collect();
    assert_eq!(w[0]["widget"]["widget_type"], "metrics_chart");
    assert_eq!(w[1]["widget"]["widget_type"], "interactive_form");
    let form_id = w[1]["call_id"].as_str().unwrap().to_string();

    let d = s
        .wait_for(&id, |d| status(d) == "idle" && raw_len(d) == 5)
        .await;
    let rs = results(&d);
    assert!(rs.iter().all(|r| r["is_error"] == false), "{rs:?}");
    assert!(
        rs[1]["content"]
            .as_str()
            .unwrap()
            .contains(&format!("[widget_response id={form_id}"))
    );

    // A form submission is an ordinary appended user message.
    let reply = format!(
        "[widget_response id={form_id} type=interactive_form]\n{{\"city\":\"Kyoto\",\"days\":3}}"
    );
    s.steer(&id, &reply).await;
    let d = s
        .wait_for(&id, |d| status(d) == "idle" && raw_len(d) == 7)
        .await;
    assert_eq!(d["raw"][5]["content"][0]["text"], reply.as_str());

    // Invalid payloads come back to the model as errors it can fix.
    s.steer(&id, "widget:pie_chart").await;
    let d = s
        .wait_for(&id, |d| status(d) == "idle" && raw_len(d) == 11)
        .await;
    let last = results(&d).last().unwrap().clone();
    assert_eq!(last["is_error"], true);
    assert!(
        last["content"]
            .as_str()
            .unwrap()
            .contains("unknown widget_type")
    );

    // Restart: widgets are restored from the history exactly as declared.
    let before = widget_calls(&d);
    s.stop().await;
    let s = Server::start(store, &ws, vec![]).await;
    let d = s.get(&format!("/sessions/{id}")).await;
    assert_eq!(widget_calls(&d), before);
    assert_eq!(
        widget_calls(&d)[0]["arguments"]["data"]["kpis"][0]["label"],
        "Requests"
    );
    s.stop().await;
}

#[tokio::test]
async fn approval_diff_always_asks_and_maps_to_approval() {
    let store = Arc::new(SqliteSessionStore::in_memory().await.unwrap());
    let s = Server::start(store, &temp_dir("ws-d"), vec![]).await;
    // Even with auto_all, the approval card waits for a human.
    let id = s
        .create(
            json!({ "provider_id": "mock", "builtin_tools": false, "approval_policy": "auto_all" }),
        )
        .await;
    s.steer(&id, "widget:approval_diff").await;
    let d = s.wait_for(&id, |d| status(d) == "needs_approval").await;
    let call = &d["pending_approvals"][0]["call"];
    assert_eq!(call["arguments"]["widget_type"], "approval_diff");
    assert!(
        call["arguments"]["data"]["diff"]
            .as_str()
            .unwrap()
            .contains("+fn bar()")
    );
    s.ok(
        "POST",
        &format!("/sessions/{id}/input"),
        Some(json!({ "type": "approval", "call_id": call["id"], "decision": "approve" })),
    )
    .await;
    let d = s.wait_for(&id, |d| status(d) == "idle").await;
    assert!(
        results(&d)[0]["content"]
            .as_str()
            .unwrap()
            .contains("approved")
    );

    // Cancel → error result with the reason.
    s.steer(&id, "widget:approval_diff").await;
    let d = s.wait_for(&id, |d| status(d) == "needs_approval").await;
    let call_id = d["pending_approvals"][0]["call"]["id"].clone();
    s.ok(
        "POST",
        &format!("/sessions/{id}/input"),
        Some(json!({ "type": "approval", "call_id": call_id, "decision": "deny", "reason": "not now" })),
    )
    .await;
    let d = s
        .wait_for(&id, |d| status(d) == "idle" && results(d).len() == 2)
        .await;
    let r = &results(&d)[1];
    assert_eq!(r["is_error"], true);
    assert!(r["content"].as_str().unwrap().contains("not now"));
    s.stop().await;
}
