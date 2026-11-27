use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use mockito::Matcher;
use oort_agent::{ToolContext, ToolError, ToolExecutor};
use oort_llm::ToolCall;
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

use crate::*;

type Handler = dyn Fn(&str, &Value) -> Option<Result<Value, RpcError>> + Send + Sync;

/// In-process transport: `handler` answers requests; `None` means "never answer".
struct MockTransport {
    handler: Box<Handler>,
    notifications: Mutex<Vec<(String, Value)>>,
}

impl MockTransport {
    fn new(
        handler: impl Fn(&str, &Value) -> Option<Result<Value, RpcError>> + Send + Sync + 'static,
    ) -> Arc<Self> {
        Arc::new(Self {
            handler: Box::new(handler),
            notifications: Mutex::default(),
        })
    }

    fn notified(&self, method: &str) -> Vec<Value> {
        self.notifications
            .lock()
            .unwrap()
            .iter()
            .filter(|(m, _)| m == method)
            .map(|(_, p)| p.clone())
            .collect()
    }
}

#[async_trait]
impl McpTransport for MockTransport {
    async fn request(
        &self,
        _id: u64,
        method: &str,
        params: Option<Value>,
    ) -> Result<Result<Value, RpcError>, TransportError> {
        match (self.handler)(method, &params.unwrap_or(Value::Null)) {
            Some(r) => Ok(r),
            None => std::future::pending().await,
        }
    }

    async fn notify(&self, method: &str, params: Option<Value>) -> Result<(), TransportError> {
        self.notifications
            .lock()
            .unwrap()
            .push((method.into(), params.unwrap_or(Value::Null)));
        Ok(())
    }

    async fn close(&self) {}
}

fn server(method: &str, params: &Value) -> Option<Result<Value, RpcError>> {
    Some(Ok(match method {
        "initialize" => {
            json!({ "protocolVersion": "2025-06-18", "capabilities": {}, "serverInfo": { "name": "mock" } })
        }
        "tools/list" => match params["cursor"].as_str() {
            None => json!({
                "tools": [
                    { "name": "write", "inputSchema": { "type": "object", "properties": { "z": {}, "a": {} } } },
                    { "name": "read", "description": "r", "inputSchema": { "type": "object" }, "annotations": { "readOnlyHint": true } }
                ],
                "nextCursor": "p2"
            }),
            Some(_) => json!({ "tools": [
                { "name": "list", "inputSchema": null, "annotations": { "readOnlyHint": false } },
                { "name": "hang", "inputSchema": { "type": "object" } }
            ] }),
        },
        "tools/call" => match params["name"].as_str().unwrap() {
            "read" => {
                json!({ "content": [{ "type": "text", "text": "hello" }, { "type": "image", "mimeType": "image/png", "data": "..." }] })
            }
            "write" => {
                json!({ "content": [{ "type": "text", "text": "disk full" }], "isError": true })
            }
            "list" => json!({ "content": [], "structuredContent": { "b": 1, "a": 2 } }),
            "bad_args" => {
                return Some(Err(RpcError {
                    code: RpcError::INVALID_PARAMS,
                    message: "missing path".into(),
                    data: None,
                }));
            }
            "hang" => return None,
            _ => {
                return Some(Err(RpcError {
                    code: -32000,
                    message: "internal".into(),
                    data: None,
                }));
            }
        },
        _ => {
            return Some(Err(RpcError {
                code: RpcError::METHOD_NOT_FOUND,
                message: "nope".into(),
                data: None,
            }));
        }
    }))
}

async fn executor(timeout: Duration) -> (Arc<MockTransport>, McpToolExecutor) {
    let t = MockTransport::new(server);
    let opts = ClientOptions {
        request_timeout: timeout,
        ..Default::default()
    };
    let client = Arc::new(McpClient::connect("mock", t.clone(), opts).await.unwrap());
    (t.clone(), McpToolExecutor::new(client).await.unwrap())
}

fn ctx() -> ToolContext {
    ToolContext {
        call_id: "1".into(),
        cancel: CancellationToken::new(),
    }
}

fn call(name: &str) -> ToolCall {
    ToolCall::new("1", name, json!({}))
}

#[tokio::test]
async fn list_is_paginated_sorted_deterministic_and_maps_read_only() {
    let (t, ex) = executor(Duration::from_secs(5)).await;
    assert_eq!(t.notified("notifications/initialized").len(), 1);
    let tools = ex.tools();
    let names: Vec<String> = tools.iter().map(|t| t.definition().name).collect();
    assert_eq!(names, ["hang", "list", "read", "write"]);
    let ro: Vec<bool> = tools.iter().map(|t| t.is_read_only()).collect();
    assert_eq!(
        ro,
        [false, false, true, false],
        "missing readOnlyHint is treated as mutating"
    );

    // Schema keys are re-serialized in sorted order regardless of server order;
    // a null schema is normalized to an object schema.
    let write = serde_json::to_string(&tools[3].definition()).unwrap();
    assert!(write.contains(r#""properties":{"a":{},"z":{}}"#), "{write}");
    assert_eq!(
        tools[1].definition().parameters,
        json!({ "type": "object" })
    );

    // Re-listing produces byte-identical definitions (safe to freeze).
    let (_, again) = executor(Duration::from_secs(5)).await;
    let bytes = |e: &McpToolExecutor| {
        serde_json::to_vec(&e.tools().iter().map(|t| t.definition()).collect::<Vec<_>>()).unwrap()
    };
    assert_eq!(bytes(&ex), bytes(&again));
}

#[tokio::test]
async fn results_and_errors_map_to_tool_outputs() {
    let (_, ex) = executor(Duration::from_secs(5)).await;

    let ok = ex.execute(&call("read"), ctx()).await.unwrap();
    assert!(!ok.is_error);
    assert_eq!(ok.content, "hello\n[image: image/png]");

    let is_error = ex.execute(&call("write"), ctx()).await.unwrap();
    assert!(
        is_error.is_error,
        "isError: true becomes an error result for the model"
    );
    assert_eq!(is_error.content, "disk full");

    let structured = ex.execute(&call("list"), ctx()).await.unwrap();
    assert_eq!(structured.content, r#"{"a":2,"b":1}"#);

    assert!(
        matches!(ex.execute(&call("bad_args"), ctx()).await, Err(ToolError::InvalidArguments(m)) if m == "missing path")
    );
    assert!(
        matches!(ex.execute(&call("other"), ctx()).await, Err(ToolError::Failed(m)) if m.contains("-32000"))
    );
}

#[tokio::test]
async fn timeout_and_cancel_send_cancellation_notifications() {
    let (t, ex) = executor(Duration::from_millis(50)).await;
    let err = ex.execute(&call("hang"), ctx()).await.unwrap_err();
    assert!(
        matches!(&err, ToolError::Failed(m) if m.contains("timeout")),
        "{err}"
    );
    assert_eq!(
        t.notified("notifications/cancelled")[0]["reason"],
        "timeout"
    );

    let (t, ex) = executor(Duration::from_secs(30)).await;
    let c = ctx();
    let cancel = c.cancel.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(30)).await;
        cancel.cancel();
    });
    assert!(matches!(
        ex.execute(&call("hang"), c).await,
        Err(ToolError::Cancelled)
    ));
    let n = t.notified("notifications/cancelled");
    assert_eq!(n[0]["reason"], "cancelled by client");
    assert!(n[0]["requestId"].is_u64());
}

#[tokio::test]
async fn streamable_http_json_sse_session_and_bearer() {
    let mut s = mockito::Server::new_async().await;
    let init = s
        .mock("POST", "/mcp")
        .match_header("authorization", "Bearer secret")
        .match_header("x-tenant", "t1")
        .match_header("accept", "application/json, text/event-stream")
        .match_body(Matcher::PartialJson(json!({ "method": "initialize" })))
        .with_header("content-type", "application/json")
        .with_header("mcp-session-id", "sess-42")
        .with_body(r#"{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":"2025-06-18","capabilities":{},"serverInfo":{"name":"http-mock"}}}"#)
        .create_async()
        .await;
    let initialized = s
        .mock("POST", "/mcp")
        .match_header("mcp-session-id", "sess-42")
        .match_header("mcp-protocol-version", "2025-06-18")
        .match_body(Matcher::PartialJson(
            json!({ "method": "notifications/initialized" }),
        ))
        .with_status(202)
        .create_async()
        .await;
    // The response arrives on an SSE stream after an unrelated notification.
    let list = s
        .mock("POST", "/mcp")
        .match_header("mcp-session-id", "sess-42")
        .match_body(Matcher::PartialJson(json!({ "method": "tools/list" })))
        .with_header("content-type", "text/event-stream")
        .with_body(concat!(
            "event: message\ndata: {\"jsonrpc\":\"2.0\",\"method\":\"notifications/message\",\"params\":{}}\n\n",
            "event: message\ndata: {\"jsonrpc\":\"2.0\",\"id\":2,\"result\":{\"tools\":[",
            "{\"name\":\"b\",\"inputSchema\":{\"type\":\"object\"}},",
            "{\"name\":\"a\",\"inputSchema\":{\"type\":\"object\"},\"annotations\":{\"readOnlyHint\":true}}]}}\n\n",
        ))
        .create_async()
        .await;
    let delete = s
        .mock("DELETE", "/mcp")
        .match_header("mcp-session-id", "sess-42")
        .create_async()
        .await;

    let transport = Arc::new(
        StreamableHttpTransport::new(HttpServerConfig {
            url: format!("{}/mcp", s.url()),
            headers: [("x-tenant".to_string(), "t1".to_string())].into(),
            token: Some(Arc::new(StaticToken("secret".into()))),
        })
        .unwrap(),
    );
    let client = Arc::new(
        McpClient::connect("http", transport, ClientOptions::default())
            .await
            .unwrap(),
    );
    assert_eq!(client.server_info().server_info.name, "http-mock");
    let ex = McpToolExecutor::new(client.clone()).await.unwrap();
    let names: Vec<String> = ex.tools().iter().map(|t| t.definition().name).collect();
    assert_eq!(names, ["a", "b"]);
    assert!(ex.tools()[0].is_read_only());
    client.close().await;

    for m in [init, initialized, list, delete] {
        m.assert_async().await;
    }
}

#[tokio::test]
async fn http_errors_are_classified() {
    let mut s = mockito::Server::new_async().await;
    let _m = s
        .mock("POST", "/mcp")
        .with_status(401)
        .expect_at_least(2)
        .create_async()
        .await;
    let transport = Arc::new(
        StreamableHttpTransport::new(HttpServerConfig {
            url: format!("{}/mcp", s.url()),
            token: Some(Arc::new(StaticToken("expired".into()))),
            ..Default::default()
        })
        .unwrap(),
    );
    let err = McpClient::connect("http", transport, ClientOptions::default())
        .await
        .err()
        .unwrap();
    // One invalidate+retry, then a classified error.
    assert!(
        matches!(err, McpError::Transport(TransportError::Unauthorized)),
        "{err}"
    );

    let _m = s
        .mock("POST", "/down")
        .with_status(503)
        .with_body("maintenance")
        .create_async()
        .await;
    let t = Arc::new(
        StreamableHttpTransport::new(HttpServerConfig {
            url: format!("{}/down", s.url()),
            ..Default::default()
        })
        .unwrap(),
    );
    let err = McpClient::connect("http", t, ClientOptions::default())
        .await
        .err()
        .unwrap();
    assert!(
        matches!(&err, McpError::Transport(e @ TransportError::Http { status: 503, .. }) if e.kind() == "http"),
        "{err}"
    );
}

#[tokio::test]
async fn mcp_tools_work_inside_the_agent_loop() {
    use oort_agent::testing::{MockModel, MockTurn};
    use oort_agent::{Agent, CompositeToolExecutor, Inbox, Namespace};
    use oort_llm::{ContentPart, Message};

    let (_, ex) = executor(Duration::from_secs(5)).await;
    let composite = CompositeToolExecutor::builder()
        .add(Namespace::Prefix("fs".into()), Arc::new(ex))
        .build()
        .unwrap();
    let model = MockModel::new(vec![
        MockTurn::tools(&[("c1", "fs__read", "{}"), ("c2", "fs__write", "{}")], 10),
        MockTurn::text("done", 10),
    ]);
    let agent = Agent::builder(model.clone(), Arc::new(composite))
        .build()
        .unwrap();
    let mut state = agent.new_state("sys", vec![Message::user("go")]);
    agent
        .run_loop(&mut state, &mut Inbox::detached())
        .await
        .unwrap();

    let reqs = model.requests();
    let names: Vec<&str> = reqs[0].tools.iter().map(|t| t.name.as_str()).collect();
    assert_eq!(names, ["fs__hang", "fs__list", "fs__read", "fs__write"]);
    let results: Vec<(String, bool)> = state.messages()[2..4]
        .iter()
        .map(|m| match &m.content[0] {
            ContentPart::ToolResult(r) => (r.content.clone(), r.is_error),
            _ => unreachable!(),
        })
        .collect();
    assert_eq!(results[0], ("hello\n[image: image/png]".into(), false));
    assert_eq!(results[1], ("disk full".into(), true));
}
