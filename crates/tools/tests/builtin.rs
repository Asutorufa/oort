//! Builtin tool behavior: workspace sandbox, command timeout/cancellation, web fetch, time, plan.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use oort_agent::{PlanStatus, PlanStep, ToolContext, ToolExecutor, ToolOutput};
use oort_llm::ToolCall;
use oort_tools::builtin::utilities::{WebFetchOptions, is_blocked};
use oort_tools::builtin::workspace::CommandOptions;
use oort_tools::{BuiltinContext, Group, PlanSink, executor, preview};
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

fn temp_root(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("oort-tools-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir.canonicalize().unwrap()
}

fn ctx_with_root(root: &Path) -> BuiltinContext {
    BuiltinContext {
        workspace_root: Some(root.to_path_buf()),
        commands: CommandOptions {
            default_timeout: Duration::from_secs(10),
            max_timeout: Duration::from_secs(10),
            max_output_chars: 2000,
        },
        web: WebFetchOptions {
            allow_private: true,
            max_chars: 200,
            ..Default::default()
        },
        ..Default::default()
    }
}

async fn call(ex: &dyn ToolExecutor, name: &str, args: Value) -> Result<ToolOutput, String> {
    call_with(ex, name, args, CancellationToken::new()).await
}

async fn call_with(
    ex: &dyn ToolExecutor,
    name: &str,
    args: Value,
    cancel: CancellationToken,
) -> Result<ToolOutput, String> {
    let c = ToolCall {
        id: "c".into(),
        name: name.into(),
        arguments: args,
        signature: None,
    };
    ex.execute(
        &c,
        ToolContext {
            call_id: "c".into(),
            cancel,
        },
    )
    .await
    .map_err(|e| e.to_string())
}

#[tokio::test]
async fn workspace_sandbox_rejects_traversal_and_symlink_escapes() {
    let root = temp_root("sandbox");
    let outside = temp_root("outside");
    std::fs::write(outside.join("secret.txt"), "TOP SECRET").unwrap();
    std::fs::write(root.join("a.txt"), "one\ntwo\nthree\nfour\n").unwrap();
    std::os::unix::fs::symlink(outside.join("secret.txt"), root.join("link.txt")).unwrap();
    std::os::unix::fs::symlink(&outside, root.join("outdir")).unwrap();
    std::fs::write(root.join("bin.dat"), [0u8, 1, 2, 3]).unwrap();
    let ex = executor(&[Group::Workspace], &ctx_with_root(&root)).unwrap();

    // Reads: inside works with line ranges; escapes are rejected.
    let out = call(
        &ex,
        "read_file",
        json!({ "path": "a.txt", "offset": 2, "limit": 2 }),
    )
    .await
    .unwrap();
    assert!(
        out.content.contains("2 | two")
            && out.content.contains("3 | three")
            && !out.content.contains("4 | four"),
        "{}",
        out.content
    );
    let escapes = ["../outside/secret.txt", "link.txt", "outdir/secret.txt"];
    for p in escapes {
        let e = call(&ex, "read_file", json!({ "path": p }))
            .await
            .unwrap_err();
        assert!(
            e.contains("outside the workspace") || e.contains("No such file"),
            "{p}: {e}"
        );
    }
    let abs = call(
        &ex,
        "read_file",
        json!({ "path": outside.join("secret.txt") }),
    )
    .await
    .unwrap_err();
    assert!(abs.contains("outside the workspace"), "{abs}");
    assert!(
        call(&ex, "read_file", json!({ "path": "bin.dat" }))
            .await
            .unwrap_err()
            .contains("binary")
    );

    // Writes: nested creation works; traversal, symlinked files and symlinked dirs are refused.
    call(
        &ex,
        "write_file",
        json!({ "path": "new/dir/b.txt", "content": "hello\n" }),
    )
    .await
    .unwrap();
    assert_eq!(
        std::fs::read_to_string(root.join("new/dir/b.txt")).unwrap(),
        "hello\n"
    );
    for p in [
        "../evil.txt",
        "link.txt",
        "outdir/evil.txt",
        "new/../../evil.txt",
    ] {
        let e = call(&ex, "write_file", json!({ "path": p, "content": "x" }))
            .await
            .unwrap_err();
        assert!(
            e.contains("outside the workspace") || e.contains("symbolic link"),
            "{p}: {e}"
        );
    }
    assert_eq!(
        std::fs::read_to_string(outside.join("secret.txt")).unwrap(),
        "TOP SECRET"
    );
    assert!(!outside.join("evil.txt").exists());

    // Exact-match edits.
    call(
        &ex,
        "write_file",
        json!({ "path": "a.txt", "old_string": "two", "new_string": "TWO" }),
    )
    .await
    .unwrap();
    assert_eq!(
        std::fs::read_to_string(root.join("a.txt")).unwrap(),
        "one\nTWO\nthree\nfour\n"
    );
    let e = call(
        &ex,
        "write_file",
        json!({ "path": "a.txt", "old_string": "missing", "new_string": "x" }),
    )
    .await
    .unwrap_err();
    assert!(e.contains("not found"));

    // The approval preview is a diff of what would change, computed without writing.
    let pv = preview(
        "write_file",
        &json!({ "path": "a.txt", "old_string": "three", "new_string": "3" }),
        &ctx_with_root(&root),
    )
    .await
    .unwrap();
    let diff = pv["diff"].as_str().unwrap();
    assert!(
        diff.contains("-three") && diff.contains("+3") && diff.contains("a/a.txt"),
        "{diff}"
    );
    assert!(
        std::fs::read_to_string(root.join("a.txt"))
            .unwrap()
            .contains("three")
    );
    let pv = preview(
        "write_file",
        &json!({ "path": "../x", "content": "x" }),
        &ctx_with_root(&root),
    )
    .await
    .unwrap();
    assert_eq!(pv["kind"], "error");
}

fn alive(pid: i32) -> bool {
    // SAFETY: signal 0 only checks existence.
    unsafe { libc::kill(pid, 0) == 0 }
}

#[tokio::test]
async fn run_command_output_timeout_and_cancellation_kill_the_process_group() {
    let root = temp_root("cmd");
    let mut ctx = ctx_with_root(&root);
    let ex = executor(&[Group::Workspace], &ctx).unwrap();

    let ok = call(
        &ex,
        "run_command",
        json!({ "command": "echo hi; echo oops >&2; pwd" }),
    )
    .await
    .unwrap();
    let v: Value = serde_json::from_str(&ok.content).unwrap();
    assert_eq!(v["exit_code"], 0);
    assert_eq!(v["stdout"], format!("hi\n{}\n", root.display()));
    assert_eq!(v["stderr"], "oops\n");
    let fail = call(&ex, "run_command", json!({ "command": "exit 3" }))
        .await
        .unwrap();
    assert!(fail.is_error && fail.content.contains("\"exit_code\": 3"));

    // Timeout: the shell and its background child are killed.
    ctx.commands.max_timeout = Duration::from_secs(1);
    let ex = executor(&[Group::Workspace], &ctx).unwrap();
    let started = Instant::now();
    let t = call(
        &ex,
        "run_command",
        json!({ "command": "sleep 30 & echo $! > bg.pid; wait" }),
    )
    .await
    .unwrap();
    assert!(
        t.is_error && t.content.contains("Timed out"),
        "{}",
        t.content
    );
    assert!(started.elapsed() < Duration::from_secs(5));
    let bg: i32 = std::fs::read_to_string(root.join("bg.pid"))
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(!alive(bg), "background child killed with the group");

    // Cancellation (interrupt / stop): same.
    let cancel = CancellationToken::new();
    let c2 = cancel.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(400)).await;
        c2.cancel();
    });
    let started = Instant::now();
    let e = call_with(
        &ex,
        "run_command",
        json!({ "command": "echo $$ > sh.pid; sleep 30 & echo $! > bg2.pid; wait" }),
        cancel,
    )
    .await
    .unwrap_err();
    assert_eq!(e, "cancelled");
    assert!(started.elapsed() < Duration::from_secs(3));
    tokio::time::sleep(Duration::from_millis(200)).await;
    for f in ["sh.pid", "bg2.pid"] {
        let pid: i32 = std::fs::read_to_string(root.join(f))
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        assert!(!alive(pid), "{f} killed");
    }

    let pv = preview(
        "run_command",
        &json!({ "command": "cargo test", "timeout_secs": 99 }),
        &ctx,
    )
    .await
    .unwrap();
    assert_eq!(pv["command"], "cargo test");
    assert_eq!(pv["cwd"], root.display().to_string());
    assert_eq!(pv["timeout_secs"], 1, "capped by max_timeout");
}

#[tokio::test]
async fn workspace_group_needs_a_root() {
    assert!(executor(&[Group::Workspace], &BuiltinContext::default()).is_err());
}

async fn web_server() -> String {
    use axum::http::header;
    use axum::response::{IntoResponse, Redirect};
    use axum::routing::get;
    let long = format!(
        "<html><head><title>Long</title></head><body><p>{}</p></body></html>",
        "word ".repeat(500)
    );
    let app = axum::Router::new()
        .route("/page", get(|| async {
            ([(header::CONTENT_TYPE, "text/html; charset=utf-8")],
             "<html><head><title>Hello Page</title><script>evil()</script></head><body><h2>Section</h2><p>Some <a href=\"/next\">link</a> text.</p></body></html>")
        }))
        .route("/long", get(move || { let l = long.clone(); async move { ([(header::CONTENT_TYPE, "text/html")], l) } }))
        .route("/moved", get(|| async { Redirect::temporary("/page") }))
        .route("/json", get(|| async { axum::Json(json!({ "a": 1 })) }))
        .route("/img", get(|| async { ([(header::CONTENT_TYPE, "image/png")], vec![0u8, 1, 2]).into_response() }))
        .route("/missing", get(|| async { (axum::http::StatusCode::NOT_FOUND, "nope") }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    base
}

#[tokio::test]
async fn web_fetch_converts_trims_follows_redirects_and_blocks_private_targets() {
    let base = web_server().await;
    let root = temp_root("web");
    let ex = executor(&[Group::Utilities], &ctx_with_root(&root)).unwrap();

    let page = call(&ex, "web_fetch", json!({ "url": format!("{base}/page") }))
        .await
        .unwrap();
    assert!(
        page.content.contains("# Hello Page") && page.content.contains("## Section"),
        "{}",
        page.content
    );
    assert!(page.content.contains(&format!("[link]({base}/next)")));
    assert!(!page.content.contains("evil"));
    let moved = call(&ex, "web_fetch", json!({ "url": format!("{base}/moved") }))
        .await
        .unwrap();
    assert!(moved.content.contains("Hello Page") && moved.content.contains("/page"));
    let long = call(
        &ex,
        "web_fetch",
        json!({ "url": format!("{base}/long"), "max_chars": 500 }),
    )
    .await
    .unwrap();
    assert!(
        long.content.contains("[trimmed: showing 500 of"),
        "{}",
        long.content
    );
    assert!(
        call(&ex, "web_fetch", json!({ "url": format!("{base}/json") }))
            .await
            .unwrap()
            .content
            .contains("\"a\":1")
    );
    assert!(
        call(&ex, "web_fetch", json!({ "url": format!("{base}/img") }))
            .await
            .unwrap_err()
            .contains("not a text page")
    );
    assert!(
        call(
            &ex,
            "web_fetch",
            json!({ "url": format!("{base}/missing") })
        )
        .await
        .unwrap_err()
        .contains("404")
    );
    assert!(
        call(&ex, "web_fetch", json!({ "url": "file:///etc/passwd" }))
            .await
            .unwrap_err()
            .contains("only http")
    );

    // Default options: loopback and private addresses are refused (SSRF).
    let strict = executor(&[Group::Utilities], &BuiltinContext::default()).unwrap();
    let e = call(
        &strict,
        "web_fetch",
        json!({ "url": format!("{base}/page") }),
    )
    .await
    .unwrap_err();
    assert!(e.contains("local or private"), "{e}");
    for ip in [
        "127.0.0.1",
        "10.1.2.3",
        "172.16.0.1",
        "192.168.1.1",
        "169.254.169.254",
        "100.64.0.1",
        "0.0.0.0",
        "::1",
        "fd00::1",
        "fe80::1",
        "::ffff:127.0.0.1",
    ] {
        assert!(is_blocked(ip.parse().unwrap()), "{ip}");
    }
    for ip in ["93.184.216.34", "1.1.1.1", "2606:4700:4700::1111"] {
        assert!(!is_blocked(ip.parse().unwrap()), "{ip}");
    }
}

#[tokio::test]
async fn current_time_and_plan_updates() {
    #[derive(Default)]
    struct Sink(Mutex<Vec<Vec<PlanStep>>>);
    impl PlanSink for Sink {
        fn publish(&self, steps: Vec<PlanStep>) {
            self.0.lock().unwrap().push(steps);
        }
    }
    let sink = Arc::new(Sink::default());
    let ctx = BuiltinContext {
        plan: Some(sink.clone()),
        ..Default::default()
    };
    let ex = executor(&[Group::Utilities, Group::CoreUx], &ctx).unwrap();

    let t: Value =
        serde_json::from_str(&call(&ex, "current_time", json!({})).await.unwrap().content).unwrap();
    assert!(t["utc"].as_str().unwrap().ends_with('Z'));
    assert!(t["unix_ms"].as_i64().unwrap() > 1_700_000_000_000);
    assert!(t["utc_offset"].as_str().unwrap().contains(':'));

    let out = call(
        &ex,
        "update_plan",
        json!({ "steps": [
        { "id": "1", "title": "Read", "status": "done" },
        { "id": "2", "title": "Fix", "status": "in_progress" },
        { "id": "3", "title": "Test", "status": "pending" }
    ] }),
    )
    .await
    .unwrap();
    assert_eq!(out.content, "Plan updated: 1/3 done, now: Fix.");
    let got = sink.0.lock().unwrap().clone();
    assert_eq!(got.len(), 1);
    assert_eq!(got[0][1].status, PlanStatus::InProgress);
    assert!(
        call(&ex, "update_plan", json!({ "steps": [] }))
            .await
            .is_err()
    );
    assert!(
        call(
            &ex,
            "update_plan",
            json!({ "steps": [{ "id": "1", "title": "x", "status": "maybe" }] })
        )
        .await
        .is_err()
    );
    // ask_user never runs on its own.
    assert!(
        call(&ex, "ask_user", json!({ "question": "?" }))
            .await
            .unwrap_err()
            .contains("answered by the user")
    );
}

#[tokio::test]
async fn read_file_pagination_and_search_in_file_sandbox() {
    let root = temp_root("read-search");
    let big: String = (1..=250)
        .map(|i| {
            if i == 180 {
                format!("line {i}: NEEDLE_TOKEN_42 found here\n")
            } else {
                format!("line {i}: ordinary content\n")
            }
        })
        .collect();
    std::fs::write(root.join("big.txt"), &big).unwrap();

    let ex = executor(&[Group::Workspace], &ctx_with_root(&root)).unwrap();

    // Default pagination (limit 200) includes structured continuation hint
    let page1 = call(&ex, "read_file", json!({ "path": "big.txt" }))
        .await
        .unwrap();
    assert!(page1.content.contains("   1 | line 1:"));
    assert!(page1.content.contains(" 200 | line 200:"));
    assert!(!page1.content.contains(" 201 | line 201:"));
    assert!(page1.content.contains("offset=201"));
    assert!(page1.content.contains("search_in_file"));

    // Continue from offset=201
    let page2 = call(
        &ex,
        "read_file",
        json!({ "path": "big.txt", "offset": 201, "limit": 200 }),
    )
    .await
    .unwrap();
    assert!(page2.content.contains(" 201 | line 201:"));
    assert!(page2.content.contains(" 250 | line 250:"));
    assert!(!page2.content.contains("offset=251"));

    // search_in_file locates needle with context lines
    let found = call(
        &ex,
        "search_in_file",
        json!({ "path": "big.txt", "pattern": "NEEDLE_TOKEN_\\d+", "context_lines": 1 }),
    )
    .await
    .unwrap();
    assert!(found.content.contains("> 180 | line 180: NEEDLE_TOKEN_42"));
    assert!(found.content.contains("  179 | line 179:"));
    assert!(found.content.contains("  181 | line 181:"));

    // Sandbox rejects path traversal
    let esc = call(
        &ex,
        "search_in_file",
        json!({ "path": "../outside.txt", "pattern": "x" }),
    )
    .await
    .unwrap_err();
    assert!(
        esc.contains("outside") || esc.contains("No such file"),
        "unexpected error: {esc}"
    );
}

#[tokio::test]
async fn spill_to_storage_and_read_tool_output_pagination_search_and_isolation() {
    use oort_agent::{StoreError, ToolOutputBlob, ToolOutputBlobStore};
    use std::collections::HashMap;

    #[derive(Default)]
    struct MemBlobs(Mutex<HashMap<String, ToolOutputBlob>>);

    #[async_trait::async_trait]
    impl ToolOutputBlobStore for MemBlobs {
        async fn save_tool_output_blob(&self, blob: &ToolOutputBlob) -> Result<(), StoreError> {
            self.0.lock().unwrap().insert(blob.id.clone(), blob.clone());
            Ok(())
        }
        async fn get_tool_output_blob(
            &self,
            id: &str,
        ) -> Result<Option<ToolOutputBlob>, StoreError> {
            Ok(self.0.lock().unwrap().get(id).cloned())
        }
        async fn can_access_blob(
            &self,
            session_id: &str,
            blob_session_id: &str,
        ) -> Result<bool, StoreError> {
            Ok(session_id == blob_session_id
                || (session_id == "sub_1" && blob_session_id == "sess_a"))
        }
    }

    let blobs = Arc::new(MemBlobs::default());
    let root = temp_root("spill");
    let mut ctx = ctx_with_root(&root);
    ctx.output_blobs = Some(blobs.clone());
    ctx.session_id = Some("sess_a".into());

    let ex = executor(&[Group::Utilities, Group::Workspace], &ctx).unwrap();

    // 1. run_command producing > 12,000 chars spills to storage and preserves head + tail
    let cmd_out = call(
        &ex,
        "run_command",
        json!({ "command": "python3 -c 'print(\"HEAD_MARKER\"); [print(f\"row_{i}: \" + \"x\"*80) for i in range(250)]; print(\"MIDDLE_SECRET_999\"); [print(f\"row2_{i}: \" + \"y\"*80) for i in range(100)]; print(\"TAIL_ERROR_MARKER\")'" }),
    )
    .await
    .unwrap();
    assert!(cmd_out.content.contains("HEAD_MARKER"));
    assert!(cmd_out.content.contains("TAIL_ERROR_MARKER"));
    assert!(cmd_out.content.contains("output_id=\"out_"));

    let id_start = cmd_out.content.find("output_id=\"").unwrap() + "output_id=\"".len();
    let id_end = cmd_out.content[id_start..].find('"').unwrap() + id_start;
    let output_id = cmd_out.content[id_start..id_end].to_string();

    // 2. read_tool_output keyword search finds MIDDLE_SECRET_999
    let searched = call(
        &ex,
        "read_tool_output",
        json!({ "output_id": output_id, "query": "MIDDLE_SECRET_999" }),
    )
    .await
    .unwrap();
    assert!(searched.content.contains("MIDDLE_SECRET_999"));

    // 3. read_tool_output pagination works
    let page = call(
        &ex,
        "read_tool_output",
        json!({ "output_id": output_id, "offset": 4000, "limit": 2000 }),
    )
    .await
    .unwrap();
    assert!(page.content.contains("Showing chars 4000-6000"));

    // 4. Subagent of sess_a can read it
    let mut sub_ctx = ctx.clone();
    sub_ctx.session_id = Some("sub_1".into());
    let sub_ex = executor(&[Group::Utilities], &sub_ctx).unwrap();
    assert!(
        call(
            &sub_ex,
            "read_tool_output",
            json!({ "output_id": output_id, "offset": 0, "limit": 200 }),
        )
        .await
        .is_ok()
    );

    // 5. Another session sess_b is denied cross-session access
    let mut other_ctx = ctx.clone();
    other_ctx.session_id = Some("sess_b".into());
    let other_ex = executor(&[Group::Utilities], &other_ctx).unwrap();
    let denied = call(
        &other_ex,
        "read_tool_output",
        json!({ "output_id": output_id, "offset": 0, "limit": 200 }),
    )
    .await
    .unwrap_err();
    assert!(denied.contains("access denied"), "{denied}");
}
