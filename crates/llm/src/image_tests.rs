//! Exact wire shape of several images in one user turn, for all four protocols.

use serde_json::{Value, json};

use crate::{
    AnthropicClient, ChatRequest, ContentPart, GeminiClient, ImageSource, Message, OpenAIClient,
    OpenAIResponsesClient,
};

fn request() -> ChatRequest {
    ChatRequest::new(vec![Message::user_parts(vec![
        ContentPart::image_base64("image/png", "AAAA"),
        ContentPart::image_base64("image/jpeg", "BBBB"),
        ContentPart::text("compare"),
    ])])
}

fn body<T: serde::Serialize>(b: T) -> Value {
    serde_json::to_value(b).unwrap()
}

#[test]
fn openai_responses_images() {
    let c = OpenAIResponsesClient::new("k", "m");
    let b = body(c.build_request_body(&request(), false).unwrap());
    assert_eq!(
        b["input"][0]["content"],
        json!([
            { "type": "input_image", "image_url": "data:image/png;base64,AAAA", "detail": "auto" },
            { "type": "input_image", "image_url": "data:image/jpeg;base64,BBBB", "detail": "auto" },
            { "type": "input_text", "text": "compare" },
        ])
    );
}

#[test]
fn openai_chat_images() {
    let c = OpenAIClient::new("k", "m");
    let b = body(c.build_request_body(&request(), false).unwrap());
    assert_eq!(
        b["messages"][0]["content"],
        json!([
            { "type": "image_url", "image_url": { "url": "data:image/png;base64,AAAA", "detail": "auto" } },
            { "type": "image_url", "image_url": { "url": "data:image/jpeg;base64,BBBB", "detail": "auto" } },
            { "type": "text", "text": "compare" },
        ])
    );
}

#[test]
fn anthropic_images() {
    let c = AnthropicClient::new("k", "m");
    let b = body(c.build_request_body(&request(), false).unwrap());
    let content = &b["messages"][0]["content"];
    assert_eq!(
        content[0],
        json!({ "type": "image", "source": { "type": "base64", "media_type": "image/png", "data": "AAAA" } })
    );
    assert_eq!(content[1]["source"]["media_type"], "image/jpeg");
    assert_eq!(content[1]["source"]["data"], "BBBB");
    assert_eq!(content[2]["text"], "compare");
}

#[test]
fn gemini_images() {
    let c = GeminiClient::new("k", "m");
    let b = body(c.build_request_body(&request()).unwrap().1);
    assert_eq!(
        b["contents"][0]["parts"],
        json!([
            { "inlineData": { "mimeType": "image/png", "data": "AAAA" } },
            { "inlineData": { "mimeType": "image/jpeg", "data": "BBBB" } },
            { "text": "compare" },
        ])
    );
}

/// History stores references; one that was never inlined must not reach a provider.
#[test]
fn unresolved_attachments_are_not_sent() {
    let part = ContentPart::Image {
        source: ImageSource::Attachment {
            attachment_id: "att_1".into(),
            media_type: "image/png".into(),
            filename: None,
            width: None,
            height: None,
        },
    };
    let req = ChatRequest::new(vec![Message::user_parts(vec![
        part,
        ContentPart::text("hi"),
    ])]);
    for b in [
        body(
            OpenAIClient::new("k", "m")
                .build_request_body(&req, false)
                .unwrap(),
        ),
        body(
            OpenAIResponsesClient::new("k", "m")
                .build_request_body(&req, false)
                .unwrap(),
        ),
        body(
            AnthropicClient::new("k", "m")
                .build_request_body(&req, false)
                .unwrap(),
        ),
        body(
            GeminiClient::new("k", "m")
                .build_request_body(&req)
                .unwrap(),
        ),
    ] {
        assert!(!b.to_string().contains("att_1"), "{b}");
    }
}
