use serde_json::json;

use crate::types::{
    ChatRequest, Citation, ContentPart, FileSearchToolConfig, Message, ReasoningEffort,
    ServerToolConfig, ThinkingConfig, ToolChoice, Vendor, WebSearchToolConfig,
};
use crate::{AnthropicClient, GeminiClient, LlmClient, OpenAIClient, OpenAIResponsesClient};
use mockito::Server;

fn full_server_tools() -> ServerToolConfig {
    ServerToolConfig {
        web_search: Some(WebSearchToolConfig {
            enabled: true,
            max_uses: Some(5),
            allowed_domains: vec![],
            blocked_domains: vec![],
        }),
        url_context: true,
        code_execution: true,
        file_search: Some(FileSearchToolConfig {
            enabled: true,
            vector_store_ids: vec!["vs_123".into()],
        }),
    }
}

#[test]
fn openai_responses_and_chat_server_tools_request_bodies() {
    let req = ChatRequest::new(vec![Message::user("latest rust release")])
        .with_server_tools(full_server_tools());

    let resp_client = OpenAIResponsesClient::new("sk-test", "gpt-5.1").with_vendor(Vendor::Openai);
    let body = resp_client.build_request_json(&req, true).unwrap();
    let tools = body["tools"].as_array().unwrap();
    assert!(tools.iter().any(|t| t["type"] == "web_search"));
    assert!(
        tools
            .iter()
            .any(|t| t["type"] == "code_interpreter" && t["container"]["type"] == "auto")
    );
    assert!(
        tools
            .iter()
            .any(|t| t["type"] == "file_search" && t["vector_store_ids"] == json!(["vs_123"]))
    );

    let chat_client = OpenAIClient::new("sk-test", "gpt-4o-search-preview").with_vendor(Vendor::Openai);
    let chat_body = chat_client.build_request_json(&req, false).unwrap();
    assert_eq!(
        chat_body["web_search_options"],
        json!({ "search_context_size": "medium" })
    );
}

#[test]
fn anthropic_server_tools_and_replay_preserves_encrypted_fields() {
    let req = ChatRequest::new(vec![Message::user("search and run python")])
        .with_server_tools(full_server_tools());
    let client = AnthropicClient::new("sk-ant", "claude-sonnet-4-5").with_vendor(Vendor::Anthropic);
    let body = client.build_request_json(&req, true).unwrap();
    let tools = body["tools"].as_array().unwrap();
    assert!(
        tools
            .iter()
            .any(|t| t["type"] == "web_search_20250305" && t["name"] == "web_search" && t["max_uses"] == 5)
    );
    assert!(
        tools
            .iter()
            .any(|t| t["type"] == "web_fetch_20250910" && t["name"] == "web_fetch")
    );
    assert!(
        tools
            .iter()
            .any(|t| t["type"] == "code_execution_20250825" && t["name"] == "code_execution")
    );

    // Multi-turn replay preserves raw_provider_payload (encrypted_content, signature, encrypted_index)
    let raw_use = json!({
        "type": "server_tool_use",
        "id": "srvtoolu_1",
        "name": "web_search",
        "input": { "query": "oort" }
    });
    let raw_res = json!({
        "type": "web_search_tool_result",
        "tool_use_id": "srvtoolu_1",
        "content": [{ "type": "web_search_result", "encrypted_content": "ENC_PAYLOAD_XYZ" }],
        "signature": "SIG_999"
    });
    let raw_cite = json!({
        "type": "web_search_result_location",
        "url": "https://example.com/a",
        "title": "Example A",
        "encrypted_index": "ENC_IDX_1"
    });
    let assistant_msg = Message::assistant_parts(vec![
        ContentPart::ServerToolUse {
            id: "srvtoolu_1".into(),
            name: "web_search".into(),
            action: None,
            input: json!({ "query": "oort" }),
            raw_provider_payload: Some(raw_use.clone()),
        },
        ContentPart::ServerToolResult {
            tool_use_id: "srvtoolu_1".into(),
            name: "web_search_tool_result".into(),
            summary: "1 results".into(),
            is_error: false,
            raw_provider_payload: Some(raw_res.clone()),
        },
        ContentPart::Text {
            text: "Found it.".into(),
            citations: vec![Citation {
                url: "https://example.com/a".into(),
                title: "Example A".into(),
                snippet: None,
                domain: Some("example.com".into()),
                raw_provider_payload: Some(raw_cite.clone()),
            }],
        },
    ]);
    let replay_req = ChatRequest::new(vec![
        Message::user("search oort"),
        assistant_msg,
        Message::user("follow up"),
    ]);
    let replay_body = client.build_request_json(&replay_req, false).unwrap();
    let assistant_blocks = replay_body["messages"][1]["content"].as_array().unwrap();
    assert_eq!(assistant_blocks[0], raw_use);
    assert_eq!(assistant_blocks[1], raw_res);
    assert_eq!(assistant_blocks[2]["citations"][0]["encrypted_index"], "ENC_IDX_1");

    // Cross-vendor switch to OpenAI Responses safely omits Anthropic raw blocks without crashing
    let openai_resp = OpenAIResponsesClient::new("sk-test", "gpt-5.1").with_vendor(Vendor::Openai);
    let cross_body = openai_resp.build_request_json(&replay_req, false).unwrap();
    let input = cross_body["input"].as_array().unwrap();
    assert!(input.iter().all(|item| item["type"] != "web_search_tool_result"));
}

#[test]
fn deepseek_three_protocols_request_bodies_and_tool_choice_guard() {
    let req = ChatRequest::new(vec![Message::user("search deepseek")])
        .with_server_tools(full_server_tools())
        .with_thinking(ThinkingConfig::with_effort(ReasoningEffort::High))
        .with_tool_choice(ToolChoice::Tool {
            name: "deepseek_web_search".into(),
        });

    // 1. Responses API
    let ds_resp = OpenAIResponsesClient::new("sk-ds", "deepseek-flash").with_vendor(Vendor::Deepseek);
    let b1 = ds_resp.build_request_json(&req, true).unwrap();
    let t1 = b1["tools"].as_array().unwrap();
    assert!(t1.iter().any(|t| t == &json!({ "type": "deepseek_web_search" })));
    assert!(t1.iter().any(|t| t == &json!({ "type": "deepseek_read_url" })));
    assert_eq!(b1["reasoning"]["effort"], "high");
    assert!(b1.get("tool_choice").is_none(), "tool_choice must not target deepseek_web_search");

    // 2. Anthropic API (must include `name`)
    let ds_ant = AnthropicClient::new("sk-ds", "deepseek-flash").with_vendor(Vendor::Deepseek);
    let b2 = ds_ant.build_request_json(&req, true).unwrap();
    let t2 = b2["tools"].as_array().unwrap();
    assert!(t2.iter().any(|t| t == &json!({ "type": "deepseek_web_search", "name": "deepseek_web_search" })));
    assert!(t2.iter().any(|t| t == &json!({ "type": "deepseek_read_url", "name": "deepseek_read_url" })));
    assert_eq!(b2["thinking"], json!({ "type": "enabled" }));
    assert!(b2.get("tool_choice").is_none());

    // 3. Chat Completions API
    let ds_chat = OpenAIClient::new("sk-ds", "deepseek-flash").with_vendor(Vendor::Deepseek);
    let b3 = ds_chat.build_request_json(&req, true).unwrap();
    let t3 = b3["tools"].as_array().unwrap();
    assert!(t3.iter().any(|t| t == &json!({ "type": "deepseek_web_search" })));
    assert!(t3.iter().any(|t| t == &json!({ "type": "deepseek_read_url" })));
    assert_eq!(b3["thinking"], json!({ "type": "enabled" }));
    assert!(b3.get("tool_choice").is_none());
}

#[test]
fn gemini_openrouter_qwen_and_custom_extra_body() {
    let req = ChatRequest::new(vec![Message::user("test")])
        .with_server_tools(full_server_tools());

    // Gemini (server tools only -> no toolConfig needed)
    let gem = GeminiClient::new("gk", "gemini-2.5-flash").with_vendor(Vendor::Gemini);
    let (_, g_body) = gem.build_request_json(&req).unwrap();
    let g_tools = g_body["tools"].as_array().unwrap();
    assert!(g_tools.contains(&json!({ "google_search": {} })));
    assert!(g_tools.contains(&json!({ "url_context": {} })));
    assert!(g_tools.contains(&json!({ "code_execution": {} })));
    assert!(g_body.get("toolConfig").is_none());

    // Gemini (server tools + custom function declarations -> must enable includeServerSideToolInvocations)
    let req_with_fn = req.clone().with_tools(vec![crate::ToolDefinition {
        name: "current_time".into(),
        description: "Get current time".into(),
        parameters: json!({ "type": "object", "properties": {} }),
    }]);
    let (_, g_body_fn) = gem.build_request_json(&req_with_fn).unwrap();
    assert_eq!(
        g_body_fn["toolConfig"]["includeServerSideToolInvocations"],
        true
    );

    // OpenRouter (both protocols)
    let or_chat = OpenAIClient::new("ork", "openai/gpt-4o").with_vendor(Vendor::Openrouter);
    let or_b1 = or_chat.build_request_json(&req, false).unwrap();
    let or_t1 = or_b1["tools"].as_array().unwrap();
    assert!(or_t1.contains(&json!({ "type": "openrouter:web_search" })));
    assert!(or_t1.contains(&json!({ "type": "openrouter:web_fetch" })));

    let or_resp = OpenAIResponsesClient::new("ork", "openai/gpt-4o").with_vendor(Vendor::Openrouter);
    let or_b2 = or_resp.build_request_json(&req, false).unwrap();
    let or_t2 = or_b2["tools"].as_array().unwrap();
    assert!(or_t2.contains(&json!({ "type": "openrouter:web_search" })));
    assert!(or_t2.contains(&json!({ "type": "openrouter:web_fetch" })));

    // Qwen (Chat & Responses)
    let qwen_chat = OpenAIClient::new("qk", "qwen-max").with_vendor(Vendor::Qwen);
    let q_b1 = qwen_chat.build_request_json(&req, false).unwrap();
    assert_eq!(q_b1["enable_search"], true);
    assert_eq!(
        q_b1["search_options"],
        json!({ "enable_source": true, "enable_citation": true })
    );

    let qwen_resp = OpenAIResponsesClient::new("qk", "qwen-max").with_vendor(Vendor::Qwen);
    let q_b2 = qwen_resp.build_request_json(&req, false).unwrap();
    let q_t2 = q_b2["tools"].as_array().unwrap();
    assert!(q_t2.contains(&json!({ "type": "web_search" })));
    assert!(q_t2.contains(&json!({ "type": "web_extractor" })));
    assert!(q_t2.contains(&json!({ "type": "code_interpreter" })));

    // Custom extra_body merge
    let custom = OpenAIClient::new("ck", "custom-model")
        .with_vendor(Vendor::Custom)
        .with_extra_body(Some(json!({ "custom_flag": true, "top_k": 40 })));
    let c_body = custom.build_request_json(&req, false).unwrap();
    assert_eq!(c_body["custom_flag"], true);
    assert_eq!(c_body["top_k"], 40);
}

#[tokio::test]
async fn parse_citations_and_server_tool_blocks_across_providers() {
    let mut server = Server::new_async().await;

    // OpenAI Responses with web_search_call + url_citation
    let m1 = server
        .mock("POST", "/responses")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(r#"{
            "id": "resp_ws",
            "model": "gpt-5.1",
            "status": "completed",
            "output": [
                {
                    "type": "web_search_call",
                    "id": "ws_1",
                    "status": "completed",
                    "action": { "type": "search", "query": "rust 1.85" }
                },
                {
                    "type": "message",
                    "content": [{
                        "type": "output_text",
                        "text": "Rust 1.85 is out.",
                        "annotations": [{
                            "type": "url_citation",
                            "url": "https://blog.rust-lang.org/2025/02/20/Rust-1.85.0.html",
                            "title": "Announcing Rust 1.85.0"
                        }]
                    }]
                }
            ]
        }"#)
        .create_async()
        .await;

    let resp = OpenAIResponsesClient::new("sk", "gpt-5.1")
        .with_base_url(server.url())
        .chat(ChatRequest::new(vec![Message::user("rust?")]))
        .await
        .unwrap();
    m1.assert_async().await;
    assert!(resp.tool_calls.is_empty(), "server tool must not enter local tool_calls");
    assert_eq!(resp.server_tool_blocks.len(), 2);
    assert_eq!(resp.citations.len(), 1);
    assert_eq!(resp.citations[0].domain.as_deref(), Some("blog.rust-lang.org"));

    // Gemini with groundingMetadata
    let m2 = server
        .mock("POST", "/models/gemini-2.5-flash:generateContent")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(r#"{
            "responseId": "g_ws",
            "candidates": [{
                "content": { "role": "model", "parts": [{ "text": "Grounded answer." }] },
                "finishReason": "STOP",
                "groundingMetadata": {
                    "webSearchQueries": ["oort cloud"],
                    "groundingChunks": [
                        { "web": { "uri": "https://science.nasa.gov/oort-cloud/", "title": "Oort Cloud - NASA" } }
                    ]
                }
            }]
        }"#)
        .create_async()
        .await;

    let gresp = GeminiClient::new("gk", "gemini-2.5-flash")
        .with_base_url(server.url())
        .chat(ChatRequest::new(vec![Message::user("oort?")]))
        .await
        .unwrap();
    m2.assert_async().await;
    assert!(gresp.tool_calls.is_empty());
    assert_eq!(gresp.citations.len(), 1);
    assert_eq!(gresp.citations[0].domain.as_deref(), Some("science.nasa.gov"));
    assert_eq!(gresp.citations[0].title, "Oort Cloud - NASA");
}

#[tokio::test]
async fn gemini_stream_keeps_latest_thought_signature_and_sanitizes_concatenated_history() {
    let mut server = Server::new_async().await;
    let sse = concat!(
        "data: {\"responseId\":\"g1\",\"candidates\":[{\"content\":{\"role\":\"model\",\"parts\":[{\"text\":\"step 1\",\"thought\":true,\"thoughtSignature\":\"QUFB=\"}]}}]}\n\n",
        "data: {\"candidates\":[{\"content\":{\"role\":\"model\",\"parts\":[{\"text\":\"step 2\",\"thought\":true,\"thoughtSignature\":\"QkJC==\"}]}}]}\n\n",
        "data: {\"candidates\":[{\"content\":{\"role\":\"model\",\"parts\":[{\"text\":\"final answer\",\"thoughtSignature\":\"Q0NDQw==\"}]},\"finishReason\":\"STOP\"}]}\n\n"
    );
    let m = server
        .mock("POST", "/models/gemini-2.5-flash:streamGenerateContent")
        .match_query(mockito::Matcher::UrlEncoded("alt".into(), "sse".into()))
        .with_status(200)
        .with_header("content-type", "text/event-stream")
        .with_body(sse)
        .create_async()
        .await;

    let client = GeminiClient::new("gk", "gemini-2.5-flash").with_base_url(server.url());
    let stream = client
        .chat_stream(ChatRequest::new(vec![Message::user("hi")]))
        .await
        .unwrap();
    let resp = crate::client::collect_stream(stream).await.unwrap();
    m.assert_async().await;
    assert_eq!(resp.content, "final answer");
    assert_eq!(resp.thinking_signature.as_deref(), Some("Q0NDQw=="));

    // Also verify previously persisted concatenated signatures are sanitized on outbound encode.
    let corrupted_history = Message {
        role: crate::Role::Assistant,
        content: vec![
            ContentPart::Thinking {
                thinking: "hmm".into(),
                signature: Some("QUFB=QkJC==Q0NDQw==".into()),
            },
            ContentPart::text("ok"),
        ],
    };
    let req = ChatRequest::new(vec![
        Message::user("q1"),
        corrupted_history,
        Message::user("q2"),
    ]);
    let (_, body) = client.build_request_json(&req).unwrap();
    assert_eq!(
        body["contents"][1]["parts"][0]["thoughtSignature"],
        "Q0NDQw=="
    );
}

#[tokio::test]
async fn gemini_strict_per_part_server_tool_replay_preserves_each_signature() {
    let mut server = Server::new_async().await;
    let sse = concat!(
        "data: {\"responseId\":\"g2\",\"candidates\":[{\"content\":{\"role\":\"model\",\"parts\":[{\"text\":\"searching...\",\"thought\":true,\"thoughtSignature\":\"U0lHMTo=\"}]}}]}\n\n",
        "data: {\"candidates\":[{\"content\":{\"role\":\"model\",\"parts\":[{\"toolCall\":{\"id\":\"st_1\",\"toolType\":\"GOOGLE_SEARCH_WEB\",\"args\":{\"queries\":[\"gold price\"]}},\"thoughtSignature\":\"U0lHMjo=\"}]}}]}\n\n",
        "data: {\"candidates\":[{\"content\":{\"role\":\"model\",\"parts\":[{\"toolResponse\":{\"id\":\"st_1\",\"toolType\":\"GOOGLE_SEARCH_WEB\",\"response\":{\"results\":[{\"title\":\"Gold\"}]}}}]}}]}\n\n",
        "data: {\"candidates\":[{\"content\":{\"role\":\"model\",\"parts\":[{\"text\":\"Gold is $3665.\",\"thoughtSignature\":\"U0lHMzo=\"}]},\"finishReason\":\"STOP\"}]}\n\n"
    );
    let m = server
        .mock("POST", "/models/gemini-2.5-flash:streamGenerateContent")
        .match_query(mockito::Matcher::UrlEncoded("alt".into(), "sse".into()))
        .with_status(200)
        .with_header("content-type", "text/event-stream")
        .with_body(sse)
        .create_async()
        .await;

    let client = GeminiClient::new("gk", "gemini-2.5-flash").with_base_url(server.url());
    let stream = client
        .chat_stream(ChatRequest::new(vec![Message::user("gold price?")]))
        .await
        .unwrap();
    let resp = crate::client::collect_stream(stream).await.unwrap();
    m.assert_async().await;

    // Server tool call signature stayed on the server_tool_block, NOT overwriting thinking_signature.
    assert_eq!(resp.thinking_signature.as_deref(), Some("U0lHMzo="));
    assert_eq!(resp.server_tool_blocks.len(), 2);

    // Replay into next Gemini turn: toolCall (with its own U0lHMjo= signature) and toolResponse
    // must be replayed verbatim as Gemini parts.
    let assistant_msg = resp.into_assistant_message();
    let next_req = ChatRequest::new(vec![
        Message::user("gold price?"),
        assistant_msg,
        Message::user("chart it"),
    ]);
    let (_, body) = client.build_request_json(&next_req).unwrap();
    let model_parts = body["contents"][1]["parts"].as_array().unwrap();
    assert!(model_parts.iter().any(|p| {
        p.get("toolCall").is_some()
            && p.get("thoughtSignature").and_then(serde_json::Value::as_str) == Some("U0lHMjo=")
    }));
    assert!(model_parts.iter().any(|p| p.get("toolResponse").is_some()));
    assert!(model_parts.iter().any(|p| {
        p.get("thought").and_then(serde_json::Value::as_bool) == Some(true)
            && p.get("thoughtSignature").and_then(serde_json::Value::as_str) == Some("U0lHMzo=")
    }));
}
