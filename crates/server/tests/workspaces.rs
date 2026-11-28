//! `/api/fs/list` and `/api/workspaces/recent`.

mod common;

use std::sync::Arc;

use common::{Server, temp_dir};
use oort_storage::SqliteSessionStore;
use serde_json::{Value, json};

async fn server(ws: &std::path::Path) -> Server {
    let store = Arc::new(SqliteSessionStore::in_memory().await.unwrap());
    Server::start(store, ws, vec![]).await
}

fn names(listing: &Value) -> Vec<&str> {
    listing["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["name"].as_str().unwrap())
        .collect()
}

async fn list(s: &Server, query: &[(&str, &str)]) -> Value {
    reqwest::Client::new()
        .get(format!("{}/fs/list", s.base))
        .query(query)
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap()
}

#[tokio::test]
async fn lists_directories_only_with_filters() {
    let root = temp_dir("fs").canonicalize().unwrap();
    for dir in [
        "beta",
        "Alpha",
        "alpine",
        ".hidden",
        "repo/.git",
        "worktree",
    ] {
        std::fs::create_dir_all(root.join(dir)).unwrap();
    }
    std::fs::write(root.join("worktree/.git"), "gitdir: elsewhere").unwrap();
    std::fs::write(root.join("alfile.txt"), "x").unwrap();
    let s = server(&root).await;
    let path = root.to_str().unwrap();

    let all = list(&s, &[("path", &format!("{path}/"))]).await;
    assert_eq!(names(&all), ["Alpha", "alpine", "beta", "repo", "worktree"]);
    assert_eq!(all["current_path"], path);
    assert_eq!(all["parent_path"], root.parent().unwrap().to_str().unwrap());
    assert_eq!(
        (all["exists"].as_bool(), all["is_dir"].as_bool()),
        (Some(true), Some(true))
    );
    assert_eq!(all["truncated"], false);
    let git: Vec<bool> = all["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["is_git_repo"].as_bool().unwrap())
        .collect();
    assert_eq!(git, [false, false, false, true, true]);

    let hidden = list(&s, &[("path", path), ("include_hidden", "true")]).await;
    assert_eq!(names(&hidden)[0], ".hidden");
    assert_eq!(hidden["entries"][0]["is_hidden"], true);

    let prefixed = list(&s, &[("path", path), ("prefix", "AL")]).await;
    assert_eq!(names(&prefixed), ["Alpha", "alpine"]);

    let missing = list(&s, &[("path", &format!("{path}/nope"))]).await;
    assert_eq!(
        (missing["exists"].as_bool(), missing["is_dir"].as_bool()),
        (Some(false), Some(false))
    );
    assert!(names(&missing).is_empty());
    let file = list(&s, &[("path", &format!("{path}/alfile.txt"))]).await;
    assert_eq!(
        (file["exists"].as_bool(), file["is_dir"].as_bool()),
        (Some(true), Some(false))
    );

    for i in 0..205 {
        std::fs::create_dir(root.join("beta").join(format!("d{i:03}"))).unwrap();
    }
    let many = list(&s, &[("path", &format!("{path}/beta"))]).await;
    assert_eq!(many["truncated"], true);
    assert_eq!(names(&many).len(), 200);
    s.stop().await;
}

#[tokio::test]
async fn expands_the_home_directory() {
    let root = temp_dir("fs-home");
    let s = server(&root).await;
    let home = std::env::var("HOME").unwrap();
    let home = home.trim_end_matches('/');
    for query in [
        &[][..],
        &[("path", "")],
        &[("path", "~")],
        &[("path", "~/")],
    ] {
        let l = list(&s, query).await;
        assert_eq!(l["current_path"], home, "{query:?}");
        assert_eq!(l["home_dir"].as_str().unwrap().trim_end_matches('/'), home);
    }
    let sub = list(&s, &[("path", "~/does-not-exist-oort/")]).await;
    assert_eq!(sub["current_path"], format!("{home}/does-not-exist-oort"));
    assert_eq!(sub["exists"], false);
    s.stop().await;
}

#[tokio::test]
async fn recent_workspaces_are_recorded_starred_and_removed() {
    let root = temp_dir("recent").canonicalize().unwrap();
    for dir in ["one/.git", "two", "preset", "chat"] {
        std::fs::create_dir_all(root.join(dir)).unwrap();
    }
    let at = |name: &str| root.join(name).to_str().unwrap().to_string();
    let s = server(&root).await;
    let paths = |v: &Value| -> Vec<String> {
        v.as_array()
            .unwrap()
            .iter()
            .map(|w| w["path"].as_str().unwrap().to_string())
            .collect()
    };

    let post = |p: String| json!({ "path": p });
    let one = s
        .ok(
            "POST",
            "/workspaces/recent",
            Some(post(format!("{}/", at("one")))),
        )
        .await;
    assert_eq!(one["path"], at("one"), "trailing slash is normalized");
    assert_eq!(one["is_git_repo"], true);
    s.ok("POST", "/workspaces/recent", Some(post(at("two"))))
        .await;
    let bad = s
        .send("POST", "/workspaces/recent", Some(post(at("missing"))))
        .await;
    assert_eq!(bad.status(), 400);

    // A preset's folder and a chat's folder are recorded too.
    let preset = s
        .ok(
            "POST",
            "/presets",
            Some(json!({ "name": "p", "provider_id": "mock", "workspace_root": at("preset") })),
        )
        .await;
    s.ok(
        "PUT",
        &format!("/presets/{}", preset["id"].as_str().unwrap()),
        Some(json!({ "description": "unchanged folder" })),
    )
    .await;
    s.create(json!({
        "provider_id": "mock", "builtin_groups": ["workspace"], "workspace_root": at("chat"),
    }))
    .await;
    let recent = s.get("/workspaces/recent").await;
    assert_eq!(
        paths(&recent),
        [at("chat"), at("preset"), at("two"), at("one")]
    );
    assert_eq!(recent[1]["use_count"], 1);

    let star = s
        .send(
            "PATCH",
            "/workspaces/recent/star",
            Some(json!({ "path": at("one"), "starred": true })),
        )
        .await;
    assert_eq!(star.status(), 204);
    std::fs::remove_dir_all(root.join("two")).unwrap();
    let recent = s.get("/workspaces/recent").await;
    assert_eq!(
        paths(&recent),
        [at("one"), at("chat"), at("preset"), at("two")]
    );
    assert_eq!(recent[0]["starred"], true);
    assert_eq!(recent[3]["exists"], false);

    // A folder that has since disappeared can still be removed.
    let gone = s
        .send("DELETE", "/workspaces/recent", Some(post(at("two"))))
        .await;
    assert_eq!(gone.status(), 204);
    assert_eq!(paths(&s.get("/workspaces/recent").await).len(), 3);
    s.stop().await;
}
