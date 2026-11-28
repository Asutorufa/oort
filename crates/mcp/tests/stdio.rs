//! End-to-end tests against the real `oort-mcp-mock` subprocess.

use std::sync::Arc;
use std::time::{Duration, Instant};

use oort_agent::{ToolContext, ToolError, ToolExecutor};
use oort_llm::ToolCall;
use oort_mcp::*;
use serde_json::json;
use tokio_util::sync::CancellationToken;

async fn connect(timeout: Duration) -> (Arc<McpClient>, McpToolExecutor) {
    let cfg = StdioServerConfig {
        command: env!("CARGO_BIN_EXE_oort-mcp-mock").into(),
        ..Default::default()
    };
    let transport = Arc::new(StdioTransport::spawn("mock", &cfg).unwrap());
    let opts = ClientOptions {
        request_timeout: timeout,
        ..Default::default()
    };
    let client = Arc::new(McpClient::connect("mock", transport, opts).await.unwrap());
    let ex = McpToolExecutor::new(client.clone()).await.unwrap();
    (client, ex)
}

fn ctx() -> ToolContext {
    ToolContext {
        call_id: "1".into(),
        cancel: CancellationToken::new(),
    }
}

#[tokio::test]
async fn stdio_list_call_and_errors() {
    let (client, ex) = connect(Duration::from_secs(5)).await;
    assert_eq!(client.server_info().server_info.name, "oort-mcp-mock");

    let defs: Vec<_> = ex
        .tools()
        .iter()
        .map(|t| (t.definition().name, t.is_read_only()))
        .collect();
    let expected = [
        ("alpha_read", true),
        ("crash", false),
        ("echo", true),
        ("fail", false),
        ("slow", true),
        ("zeta_write", false),
    ];
    assert_eq!(defs, expected.map(|(n, r)| (n.to_string(), r)));
    let alpha = serde_json::to_string(&ex.tools()[0].definition()).unwrap();
    assert!(
        alpha.contains(
            r#""properties":{"limit":{"type":"integer"},"path":{"type":"string"}},"type":"object""#
        ),
        "{alpha}"
    );

    let echo = ex
        .execute(&ToolCall::new("1", "echo", json!({ "text": "hi" })), ctx())
        .await
        .unwrap();
    assert_eq!((echo.content.as_str(), echo.is_error), ("hi", false));
    let fail = ex
        .execute(&ToolCall::new("2", "fail", json!({})), ctx())
        .await
        .unwrap();
    assert!(fail.is_error);
    let unknown = ex
        .execute(&ToolCall::new("3", "nope", json!({})), ctx())
        .await;
    assert!(matches!(unknown, Err(ToolError::InvalidArguments(_))));

    // Concurrent calls are multiplexed over one pipe by request id.
    let calls = (0..5).map(|i| {
        let ex = &ex;
        async move {
            ex.execute(
                &ToolCall::new(i.to_string(), "echo", json!({ "text": i.to_string() })),
                ctx(),
            )
            .await
        }
    });
    let outs = futures_util::future::join_all(calls).await;
    for (i, o) in outs.into_iter().enumerate() {
        assert_eq!(o.unwrap().content, i.to_string());
    }
    client.close().await;
}

#[tokio::test]
async fn stdio_cancel_timeout_and_crash() {
    let (client, ex) = connect(Duration::from_millis(200)).await;
    let started = Instant::now();
    let err = ex
        .execute(&ToolCall::new("1", "slow", json!({})), ctx())
        .await
        .unwrap_err();
    assert!(
        matches!(&err, ToolError::Failed(m) if m.contains("timeout")),
        "{err}"
    );
    assert!(started.elapsed() < Duration::from_secs(5));

    let c = ctx();
    let cancel = c.cancel.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(20)).await;
        cancel.cancel();
    });
    assert!(matches!(
        ex.execute(&ToolCall::new("2", "slow", json!({})), c).await,
        Err(ToolError::Cancelled)
    ));

    // The server dying is a classified transport error, not a hang.
    let err = ex
        .execute(&ToolCall::new("3", "crash", json!({})), ctx())
        .await
        .unwrap_err();
    assert!(
        matches!(&err, ToolError::Failed(m) if m.contains("disconnected")),
        "{err}"
    );
    let err = ex
        .execute(&ToolCall::new("4", "echo", json!({})), ctx())
        .await
        .unwrap_err();
    assert!(
        matches!(&err, ToolError::Failed(m) if m.contains("disconnected")),
        "{err}"
    );
    client.close().await;
}

#[tokio::test]
async fn config_connects_servers_with_namespaces() {
    let raw = format!(
        r#"{{"mcpServers": {{"mock": {{"command": {:?}}}, "remote": {{"url": "http://127.0.0.1:9/mcp", "bearerToken": "x"}}}}}}"#,
        env!("CARGO_BIN_EXE_oort-mcp-mock")
    );
    let mut cfg = McpConfig::from_json(&raw).unwrap();
    assert!(matches!(cfg.servers["remote"], ServerEntry::Http { .. }));
    cfg.servers.remove("remote"); // nothing listens on port 9
    let (builder, clients) = cfg
        .connect_into(
            oort_agent::CompositeToolExecutor::builder(),
            &ClientOptions::default(),
        )
        .await
        .unwrap();
    let composite = builder.build().unwrap();
    let names: Vec<String> = composite
        .tools()
        .iter()
        .map(|t| t.definition().name)
        .collect();
    assert_eq!(names[0], "mock__alpha_read");
    assert_eq!(names.len(), 6);
    for c in clients {
        c.close().await;
    }
}

fn record(command: &str) -> oort_agent::McpServerRecord {
    oort_agent::McpServerRecord {
        id: "s1".into(),
        name: "mock".into(),
        transport: oort_agent::McpTransportKind::Stdio,
        url: None,
        command: Some(command.into()),
        args: vec![],
        env: Default::default(),
        headers: Default::default(),
        auth: oort_agent::McpAuth::None,
        is_enabled: true,
        created_at: 0,
        updated_at: 0,
    }
}

#[tokio::test]
async fn probe_stored_record_lists_sorted_tools_and_disconnects() {
    let rec = record(env!("CARGO_BIN_EXE_oort-mcp-mock"));
    let p = probe(&rec, ClientOptions::default(), None).await.unwrap();
    assert_eq!(p.server.server_info.name, "oort-mcp-mock");
    let names: Vec<&str> = p.tools.iter().map(|t| t.name.as_str()).collect();
    assert_eq!(
        names,
        ["alpha_read", "crash", "echo", "fail", "slow", "zeta_write"]
    );

    // Unresolvable secrets and missing commands are configuration errors, not transport ones.
    let mut bad = rec.clone();
    bad.env
        .insert("TOKEN".into(), "env:OORT_TEST_SURELY_UNSET".into());
    assert!(matches!(
        probe(&bad, ClientOptions::default(), None).await,
        Err(McpError::Config(m)) if m.contains("OORT_TEST_SURELY_UNSET")
    ));
    let mut bad = rec;
    bad.command = None;
    assert!(matches!(
        probe(&bad, ClientOptions::default(), None).await,
        Err(McpError::Config(_))
    ));
    assert!(matches!(
        probe(
            &record("/nonexistent/oort-mcp"),
            ClientOptions::default(),
            None
        )
        .await,
        Err(McpError::Transport(_))
    ));
}
