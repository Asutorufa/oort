//! Minimal stdio MCP server for tests and demos.
//!
//! Tools (listed 2 per page to exercise pagination, deliberately unsorted):
//! `zeta_write` (side effects), `alpha_read` (readOnlyHint), `echo` (readOnlyHint),
//! `fail` (returns `isError: true`), `slow` (sleeps 30s), `crash` (exits the process).

use std::sync::Arc;
use std::time::Duration;

use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::Mutex;

fn tools() -> Vec<Value> {
    // Keys intentionally out of order: clients must not depend on server key order.
    let raw = r#"[
      {"name":"zeta_write","description":"Write a note (side effects)",
       "inputSchema":{"type":"object","required":["text"],"properties":{"text":{"type":"string"}}},
       "annotations":{"destructiveHint":true}},
      {"name":"alpha_read","description":"Read a path",
       "inputSchema":{"properties":{"path":{"type":"string"},"limit":{"type":"integer"}},"type":"object"},
       "annotations":{"readOnlyHint":true}},
      {"name":"echo","title":"Echo","inputSchema":{"type":"object","properties":{"text":{"type":"string"}}},
       "annotations":{"readOnlyHint":true}},
      {"name":"fail","description":"Always fails","inputSchema":{"type":"object"}},
      {"name":"slow","description":"Sleeps for 30 seconds","inputSchema":{"type":"object"},
       "annotations":{"readOnlyHint":true}},
      {"name":"crash","description":"Exits the server","inputSchema":{}}
    ]"#;
    serde_json::from_str(raw).unwrap()
}

type Out = Arc<Mutex<tokio::io::Stdout>>;

async fn send(out: &Out, v: Value) {
    let mut line = serde_json::to_vec(&v).unwrap();
    line.push(b'\n');
    let mut o = out.lock().await;
    let _ = o.write_all(&line).await;
    let _ = o.flush().await;
}

fn text(t: impl Into<String>, is_error: bool) -> Value {
    json!({ "content": [{ "type": "text", "text": t.into() }], "isError": is_error })
}

async fn handle(out: Out, id: Value, method: String, params: Value) {
    let result = match method.as_str() {
        "initialize" => json!({
            "protocolVersion": params["protocolVersion"],
            "capabilities": { "tools": {} },
            "serverInfo": { "name": "oort-mcp-mock", "version": "0.1.0" }
        }),
        "tools/list" => {
            let all = tools();
            let start: usize = params["cursor"]
                .as_str()
                .and_then(|c| c.parse().ok())
                .unwrap_or(0);
            let page: Vec<Value> = all.iter().skip(start).take(2).cloned().collect();
            let mut r = json!({ "tools": page });
            if start + 2 < all.len() {
                r["nextCursor"] = json!((start + 2).to_string());
            }
            r
        }
        "tools/call" => {
            let args = &params["arguments"];
            match params["name"].as_str().unwrap_or_default() {
                "echo" => text(args["text"].as_str().unwrap_or_default(), false),
                "alpha_read" => text(
                    format!("contents of {}", args["path"].as_str().unwrap_or("?")),
                    false,
                ),
                "zeta_write" => text(
                    format!("wrote: {}", args["text"].as_str().unwrap_or_default()),
                    false,
                ),
                "fail" => text("boom: the tool failed", true),
                "slow" => {
                    tokio::time::sleep(Duration::from_secs(30)).await;
                    text("finally", false)
                }
                "crash" => std::process::exit(3),
                other => {
                    send(
                        &out,
                        json!({ "jsonrpc": "2.0", "id": id,
                        "error": { "code": -32602, "message": format!("unknown tool: {other}") } }),
                    )
                    .await;
                    return;
                }
            }
        }
        "ping" => json!({}),
        other => {
            send(
                &out,
                json!({ "jsonrpc": "2.0", "id": id,
                "error": { "code": -32601, "message": format!("method not found: {other}") } }),
            )
            .await;
            return;
        }
    };
    send(
        &out,
        json!({ "jsonrpc": "2.0", "id": id, "result": result }),
    )
    .await;
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let out: Out = Arc::new(Mutex::new(tokio::io::stdout()));
    let mut lines = BufReader::new(tokio::io::stdin()).lines();
    eprintln!("oort-mcp-mock ready");
    while let Ok(Some(line)) = lines.next_line().await {
        let Ok(msg) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        let method = msg["method"].as_str().map(str::to_string);
        match (msg.get("id").cloned(), method) {
            (Some(id), Some(method)) => {
                let out = out.clone();
                let params = msg.get("params").cloned().unwrap_or(Value::Null);
                tokio::spawn(handle(out, id, method, params));
            }
            (None, Some(m)) if m == "notifications/initialized" => {
                // Exercise the client's handling of server→client traffic.
                send(
                    &out,
                    json!({ "jsonrpc": "2.0", "method": "notifications/message",
                    "params": { "level": "info", "data": "hello" } }),
                )
                .await;
                send(
                    &out,
                    json!({ "jsonrpc": "2.0", "id": "srv-1", "method": "roots/list" }),
                )
                .await;
            }
            _ => {} // responses to our server request, cancellations, other notifications
        }
    }
}
