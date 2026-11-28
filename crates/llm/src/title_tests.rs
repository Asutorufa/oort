//! DeepSeek native titles: requested only for DeepSeek, read from all three protocols.

use mockito::Server;
use serde_json::{Value, json};

use crate::{
    AnthropicClient, ChatRequest, LlmClient, Message, OpenAIClient, OpenAIResponsesClient,
    Provider, Vendor, collect_stream,
};

fn clients(base: &str, vendor: Vendor) -> [Provider; 3] {
    [
        AnthropicClient::new("k", "m")
            .with_vendor(vendor)
            .with_base_url(base)
            .into(),
        OpenAIResponsesClient::new("k", "m")
            .with_vendor(vendor)
            .with_base_url(base)
            .into(),
        OpenAIClient::new("k", "m")
            .with_vendor(vendor)
            .with_base_url(base)
            .into(),
    ]
}

fn request() -> ChatRequest {
    let mut r = ChatRequest::new(vec![Message::user("hi")]);
    r.generate_title = true;
    r
}

fn body(client: &Provider, request: &ChatRequest) -> Value {
    match client {
        Provider::Anthropic(c) => c.build_request_json(request, true),
        Provider::OpenAIResponses(c) => c.build_request_json(request, true),
        Provider::OpenAI(c) => c.build_request_json(request, true),
        Provider::Gemini(_) => unreachable!(),
    }
    .unwrap()
}

#[test]
fn title_is_requested_only_from_deepseek_and_only_when_asked() {
    for client in clients("http://x", Vendor::Deepseek) {
        let with = body(&client, &request());
        assert_eq!(with["title"], json!({ "enabled": true }), "{with}");
        // Nothing but the title parameter differs, so the cached prefix is the same.
        let mut without = body(&client, &ChatRequest::new(vec![Message::user("hi")]));
        assert!(without.get("title").is_none());
        without["title"] = json!({ "enabled": true });
        assert_eq!(with, without);
    }
    for vendor in [
        Vendor::Openai,
        Vendor::Anthropic,
        Vendor::Openrouter,
        Vendor::Qwen,
        Vendor::Ollama,
        Vendor::Custom,
    ] {
        for client in clients("http://x", vendor) {
            let b = body(&client, &request());
            assert!(b.get("title").is_none(), "{vendor:?}: {b}");
        }
    }
}

const PATHS: [&str; 3] = ["/v1/messages", "/responses", "/chat/completions"];

#[tokio::test]
async fn title_is_read_from_non_streaming_responses() {
    let replies = [
        json!({ "content": [{ "type": "text", "text": "yo" }], "title": "Greeting" }),
        json!({ "status": "completed", "title": "Greeting", "output": [
            { "type": "message", "content": [{ "type": "output_text", "text": "yo" }] }] }),
        json!({ "choices": [{ "message": { "content": "yo", "title": "Greeting" } }] }),
    ];
    for (i, reply) in replies.iter().enumerate() {
        let mut server = Server::new_async().await;
        let mock = server
            .mock("POST", PATHS[i])
            .with_header("content-type", "application/json")
            .with_body(reply.to_string())
            .create_async()
            .await;
        let client = &clients(&server.url(), Vendor::Deepseek)[i];
        let response = client.chat(request()).await.unwrap();
        mock.assert_async().await;
        assert_eq!(response.content, "yo");
        assert_eq!(response.title.as_deref(), Some("Greeting"), "{}", PATHS[i]);
    }
}

#[tokio::test]
async fn title_is_read_from_streams() {
    let sse = |events: &[Value]| {
        events
            .iter()
            .map(|e| format!("data: {e}\n\n"))
            .collect::<String>()
    };
    let streams = [
        sse(&[
            json!({ "type": "content_block_delta", "index": 0, "delta": { "type": "text_delta", "text": "yo" } }),
            json!({ "type": "message_delta", "delta": { "stop_reason": "end_turn", "title": "Greeting" } }),
            json!({ "type": "message_stop" }),
        ]),
        sse(&[
            json!({ "type": "response.output_text.delta", "delta": "yo" }),
            json!({ "type": "response.completed", "response": { "status": "completed", "title": "Greeting" } }),
        ]),
        sse(&[
            json!({ "choices": [{ "delta": { "content": "yo" } }] }),
            json!({ "choices": [{ "delta": { "title": "Greeting" }, "finish_reason": "stop" }] }),
        ]) + "data: [DONE]\n\n",
    ];
    for (i, stream) in streams.iter().enumerate() {
        let mut server = Server::new_async().await;
        server
            .mock("POST", PATHS[i])
            .with_header("content-type", "text/event-stream")
            .with_body(stream)
            .create_async()
            .await;
        let client = &clients(&server.url(), Vendor::Deepseek)[i];
        let response = collect_stream(client.chat_stream(request()).await.unwrap())
            .await
            .unwrap();
        assert_eq!(response.content, "yo");
        assert_eq!(response.title.as_deref(), Some("Greeting"), "{}", PATHS[i]);
    }
}
