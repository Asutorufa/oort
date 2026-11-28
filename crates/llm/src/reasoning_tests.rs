//! Thinking-level mapping per provider, and the sampling parameters each one refuses alongside it.

use serde_json::{Value, json};

use crate::{
    AnthropicClient, ChatRequest, GeminiClient, Message, OpenAIClient, OpenAIResponsesClient,
    ReasoningEffort, ThinkingConfig,
};

fn req(thinking: Option<ThinkingConfig>) -> ChatRequest {
    let mut r = ChatRequest::new(vec![Message::user("hi")]).with_temperature(0.3);
    r.top_p = Some(0.5);
    r.top_k = Some(20);
    r.thinking = thinking;
    r
}

fn body<T: serde::Serialize>(b: T) -> Value {
    serde_json::to_value(b).unwrap()
}

#[test]
fn anthropic_maps_effort_to_a_budget_and_drops_sampling_params() {
    let c = AnthropicClient::new("k", "claude-x");
    let b = body(
        c.build_request_body(
            &req(Some(ThinkingConfig::with_effort(ReasoningEffort::Medium))),
            false,
        )
        .unwrap(),
    );
    assert_eq!(
        b["thinking"],
        json!({ "type": "enabled", "budget_tokens": 10240 })
    );
    assert!(
        b.get("temperature").is_none() && b.get("top_k").is_none() && b.get("top_p").is_none(),
        "{b}"
    );
    assert!(b["max_tokens"].as_u64().unwrap() > 10240);

    // A max_tokens at or below the budget is raised above it.
    let mut r = req(Some(ThinkingConfig::with_effort(ReasoningEffort::High)));
    r.max_tokens = Some(2000);
    let b = body(c.build_request_body(&r, false).unwrap());
    assert!(b["max_tokens"].as_u64().unwrap() > 24576);

    // Off / unset keep the sampling parameters.
    let b = body(
        c.build_request_body(&req(Some(ThinkingConfig::default())), false)
            .unwrap(),
    );
    assert_eq!(b["thinking"], json!({ "type": "disabled" }));
    assert!((b["temperature"].as_f64().unwrap() - 0.3).abs() < 1e-6);
    let b = body(c.build_request_body(&req(None), false).unwrap());
    assert!(b.get("thinking").is_none() && b.get("temperature").is_some());
}

#[test]
fn openai_sends_reasoning_effort_without_temperature() {
    let c = OpenAIClient::new("k", "o-model");
    let b = body(
        c.build_request_body(
            &req(Some(ThinkingConfig::with_effort(ReasoningEffort::Low))),
            false,
        )
        .unwrap(),
    );
    assert_eq!(b["reasoning_effort"], "low");
    assert!(
        b.get("temperature").is_none() && b.get("top_p").is_none(),
        "{b}"
    );
    let b = body(
        c.build_request_body(&req(Some(ThinkingConfig::default())), false)
            .unwrap(),
    );
    assert!(b.get("reasoning_effort").is_none() && b.get("temperature").is_some());

    let c = OpenAIResponsesClient::new("k", "gpt-x");
    let b = body(
        c.build_request_body(
            &req(Some(ThinkingConfig::with_effort(ReasoningEffort::High))),
            false,
        )
        .unwrap(),
    );
    assert_eq!(b["reasoning"]["effort"], "high");
    assert!(b.get("temperature").is_none(), "{b}");
}

#[test]
fn gemini_uses_budgets_for_2x_and_levels_for_later_models() {
    let cfg = |model: &str, t: ThinkingConfig| {
        body(
            GeminiClient::new("k", model)
                .build_request_body(&req(Some(t)))
                .unwrap()
                .1,
        )["generationConfig"]["thinkingConfig"]
            .clone()
    };
    let e = |x| ThinkingConfig::with_effort(x);
    assert_eq!(
        cfg("gemini-2.5-flash", e(ReasoningEffort::Medium))["thinkingBudget"],
        8192
    );
    assert!(
        cfg("gemini-2.5-flash", e(ReasoningEffort::Medium))
            .get("thinkingLevel")
            .is_none()
    );
    assert_eq!(
        cfg("gemini-2.5-flash", ThinkingConfig::default())["thinkingBudget"],
        0
    );
    // 2.5 Pro cannot disable thinking: lowest budget instead.
    assert_eq!(
        cfg("gemini-2.5-pro", ThinkingConfig::default())["thinkingBudget"],
        128
    );
    assert_eq!(
        cfg("gemini-3-pro-preview", e(ReasoningEffort::High))["thinkingLevel"],
        "HIGH"
    );
    assert!(
        cfg("gemini-3-pro-preview", e(ReasoningEffort::High))
            .get("thinkingBudget")
            .is_none()
    );
    assert_eq!(
        cfg("gemini-3-flash", ThinkingConfig::default())["thinkingLevel"],
        "MINIMAL"
    );
    assert_eq!(
        cfg("gemini-3-pro", ThinkingConfig::default())["thinkingLevel"],
        "LOW"
    );
}
