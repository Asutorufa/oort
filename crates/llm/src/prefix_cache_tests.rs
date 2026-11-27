//! Dedicated test suite verifying prompt prefix cache stability across all providers.
//!
//! In LLM agent loops, even a single reordered JSON key, shifted tool definition, or mutated
//! historical message invalidates the KV / prompt prefix cache and dramatically increases cost.
//! These tests enforce byte-level determinism, append-only multi-turn prefix invariants, and
//! provider-specific cache controls across OpenAI, OpenAI Responses, Anthropic, and Gemini.

use serde_json::{Map, Value, json};

use crate::{
    AnthropicClient, CacheTtl, ChatRequest, ContentPart, GeminiClient, Message, OpenAIClient,
    OpenAIResponsesClient, PromptCacheConfig, ToolCall, ToolDefinition,
};

fn sample_tools() -> Vec<ToolDefinition> {
    // Intentionally NOT in alphabetical order to verify insertion order is strictly preserved.
    vec![
        ToolDefinition::new(
            "zebra_read_file",
            "Read a file from disk",
            json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string" },
                    "offset": { "type": "integer" },
                    "limit": { "type": "integer" }
                },
                "required": ["path"]
            }),
        ),
        ToolDefinition::new(
            "alpha_run_shell",
            "Execute a shell command",
            json!({
                "type": "object",
                "properties": {
                    "command": { "type": "string" },
                    "timeout_ms": { "type": "integer" }
                },
                "required": ["command"]
            }),
        ),
        ToolDefinition::new(
            "middle_search",
            "Search files by regex",
            json!({
                "type": "object",
                "properties": {
                    "pattern": { "type": "string" },
                    "dir": { "type": "string" }
                },
                "required": ["pattern"]
            }),
        ),
    ]
}

fn multi_turn_histories() -> (Vec<Message>, Vec<Message>, Vec<Message>, Vec<Message>) {
    // Turn 1: initial user query
    let turn1 = vec![Message::user("Inspect Cargo.toml and run tests")];

    // Turn 2: assistant thinks + calls 1 tool, tool returns result
    let mut turn2 = turn1.clone();
    turn2.push(Message::assistant_parts(vec![
        ContentPart::thinking("First read Cargo.toml.", Some("sig_turn2".to_string())),
        ContentPart::text("Reading Cargo.toml."),
        ContentPart::ToolCall(ToolCall::new(
            "call_1",
            "zebra_read_file",
            json!({"path": "Cargo.toml", "limit": 100, "offset": 0}),
        )),
    ]));
    turn2.push(Message::tool_result(
        "call_1",
        Some("zebra_read_file"),
        "[workspace]\nmembers = [\"crates/llm\"]",
        false,
    ));

    // Turn 3: assistant makes 2 parallel tool calls, both return results
    let mut turn3 = turn2.clone();
    turn3.push(Message::assistant_parts(vec![
        ContentPart::ToolCall(ToolCall::new(
            "call_2",
            "middle_search",
            json!({"pattern": "LlmClient", "dir": "crates"}),
        )),
        ContentPart::ToolCall(ToolCall::new(
            "call_3",
            "alpha_run_shell",
            json!({"command": "cargo test", "timeout_ms": 5000}),
        )),
    ]));
    turn3.push(Message::tool_result(
        "call_2",
        Some("middle_search"),
        "crates/llm/src/client.rs:16",
        false,
    ));
    turn3.push(Message::tool_result(
        "call_3",
        Some("alpha_run_shell"),
        "test result: ok.",
        false,
    ));

    // Turn 4: assistant finalizes and user asks a follow-up
    let mut turn4 = turn3.clone();
    turn4.push(Message::assistant("All tests passed."));
    turn4.push(Message::user("Now check clippy warnings."));

    (turn1, turn2, turn3, turn4)
}

fn strip_cache_control(value: &Value) -> Value {
    match value {
        Value::Object(map) => {
            let mut out = Map::new();
            for (k, v) in map {
                if k == "cache_control" {
                    continue;
                }
                out.insert(k.clone(), strip_cache_control(v));
            }
            Value::Object(out)
        }
        Value::Array(items) => Value::Array(items.iter().map(strip_cache_control).collect()),
        other => other.clone(),
    }
}

fn count_cache_control(value: &Value) -> usize {
    match value {
        Value::Object(map) => {
            let here = usize::from(map.contains_key("cache_control"));
            here + map.values().map(count_cache_control).sum::<usize>()
        }
        Value::Array(items) => items.iter().map(count_cache_control).sum(),
        _ => 0,
    }
}

fn assert_json_array_is_prefix(earlier: &Value, later: &Value, label: &str) {
    let earlier_arr = earlier
        .as_array()
        .unwrap_or_else(|| panic!("{label}: earlier value is not an array"));
    let later_arr = later
        .as_array()
        .unwrap_or_else(|| panic!("{label}: later value is not an array"));
    assert!(
        later_arr.len() >= earlier_arr.len(),
        "{label}: later array ({}) shorter than earlier ({})",
        later_arr.len(),
        earlier_arr.len()
    );

    for (idx, (a, b)) in earlier_arr.iter().zip(later_arr.iter()).enumerate() {
        let a_bytes = serde_json::to_string(a).unwrap();
        let b_bytes = serde_json::to_string(b).unwrap();
        assert_eq!(
            a_bytes, b_bytes,
            "{label}: prefix mismatch at index {idx}!\nearlier: {a_bytes}\nlater:   {b_bytes}"
        );
    }
}

#[test]
fn test_byte_for_byte_determinism_across_all_providers() {
    let (_, _, turn3, _) = multi_turn_histories();
    let req = ChatRequest::new(turn3)
        .with_system("You are a deterministic coding agent.")
        .with_tools(sample_tools())
        .with_prompt_cache(
            PromptCacheConfig::enabled()
                .with_key("session-42")
                .with_ttl(CacheTtl::OneHour),
        );

    let openai = OpenAIClient::new("k", "gpt-4o");
    let a1 = serde_json::to_string(&openai.build_request_body(&req, true).unwrap()).unwrap();
    let a2 = serde_json::to_string(&openai.build_request_body(&req, true).unwrap()).unwrap();
    assert_eq!(a1, a2, "OpenAI request serialization must be deterministic");

    let responses = OpenAIResponsesClient::new("k", "gpt-4.1");
    let r1 = serde_json::to_string(&responses.build_request_body(&req, true).unwrap()).unwrap();
    let r2 = serde_json::to_string(&responses.build_request_body(&req, true).unwrap()).unwrap();
    assert_eq!(
        r1, r2,
        "OpenAI Responses request serialization must be deterministic"
    );

    let anthropic = AnthropicClient::new("k", "claude-sonnet-4-20250514");
    let c1 = serde_json::to_string(&anthropic.build_request_body(&req, true).unwrap()).unwrap();
    let c2 = serde_json::to_string(&anthropic.build_request_body(&req, true).unwrap()).unwrap();
    assert_eq!(
        c1, c2,
        "Anthropic request serialization must be deterministic"
    );

    let gemini = GeminiClient::new("k", "gemini-2.5-pro");
    let g1 = serde_json::to_string(&gemini.build_request_body(&req).unwrap().1).unwrap();
    let g2 = serde_json::to_string(&gemini.build_request_body(&req).unwrap().1).unwrap();
    assert_eq!(g1, g2, "Gemini request serialization must be deterministic");
}

#[test]
fn test_json_object_key_insertion_order_does_not_bust_cache() {
    // Build two JSON schema objects with keys inserted in opposite orders.
    let mut map_a = Map::new();
    map_a.insert("z_param".to_string(), json!({"type": "string"}));
    map_a.insert("a_param".to_string(), json!({"type": "integer"}));
    map_a.insert("m_param".to_string(), json!({"type": "boolean"}));

    let mut map_b = Map::new();
    map_b.insert("m_param".to_string(), json!({"type": "boolean"}));
    map_b.insert("a_param".to_string(), json!({"type": "integer"}));
    map_b.insert("z_param".to_string(), json!({"type": "string"}));

    let mut args_a = Map::new();
    args_a.insert("z_param".to_string(), json!("hello"));
    args_a.insert("a_param".to_string(), json!(42));

    let mut args_b = Map::new();
    args_b.insert("a_param".to_string(), json!(42));
    args_b.insert("z_param".to_string(), json!("hello"));

    let make_req = |props: Map<String, Value>, args: Map<String, Value>| {
        ChatRequest::new(vec![
            Message::user("hi"),
            Message::assistant_parts(vec![ContentPart::ToolCall(ToolCall::new(
                "call_1",
                "demo",
                Value::Object(args),
            ))]),
            Message::tool_result("call_1", Some("demo"), "ok", false),
        ])
        .with_system("System")
        .with_tools(vec![ToolDefinition::new(
            "demo",
            "Demo tool",
            json!({"type": "object", "properties": Value::Object(props)}),
        )])
    };

    let req_a = make_req(map_a, args_a);
    let req_b = make_req(map_b, args_b);

    let openai = OpenAIClient::new("k", "gpt-4o");
    assert_eq!(
        serde_json::to_string(&openai.build_request_body(&req_a, false).unwrap()).unwrap(),
        serde_json::to_string(&openai.build_request_body(&req_b, false).unwrap()).unwrap()
    );

    let responses = OpenAIResponsesClient::new("k", "gpt-4.1");
    assert_eq!(
        serde_json::to_string(&responses.build_request_body(&req_a, false).unwrap()).unwrap(),
        serde_json::to_string(&responses.build_request_body(&req_b, false).unwrap()).unwrap()
    );

    let anthropic = AnthropicClient::new("k", "claude-sonnet-4-20250514");
    assert_eq!(
        serde_json::to_string(&anthropic.build_request_body(&req_a, false).unwrap()).unwrap(),
        serde_json::to_string(&anthropic.build_request_body(&req_b, false).unwrap()).unwrap()
    );

    let gemini = GeminiClient::new("k", "gemini-2.5-pro");
    assert_eq!(
        serde_json::to_string(&gemini.build_request_body(&req_a).unwrap().1).unwrap(),
        serde_json::to_string(&gemini.build_request_body(&req_b).unwrap().1).unwrap()
    );
}

#[test]
fn test_tool_definition_order_is_strictly_preserved_across_all_providers() {
    let req = ChatRequest::new(vec![Message::user("go")]).with_tools(sample_tools());
    let expected = vec!["zebra_read_file", "alpha_run_shell", "middle_search"];

    let openai = OpenAIClient::new("k", "gpt-4o");
    let oa_val = serde_json::to_value(openai.build_request_body(&req, false).unwrap()).unwrap();
    let oa_names: Vec<&str> = oa_val["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["function"]["name"].as_str().unwrap())
        .collect();
    assert_eq!(oa_names, expected);

    let responses = OpenAIResponsesClient::new("k", "gpt-4.1");
    let resp_val =
        serde_json::to_value(responses.build_request_body(&req, false).unwrap()).unwrap();
    let resp_names: Vec<&str> = resp_val["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    assert_eq!(resp_names, expected);

    let anthropic = AnthropicClient::new("k", "claude-sonnet-4-20250514");
    let ant_val =
        serde_json::to_value(anthropic.build_request_body(&req, false).unwrap()).unwrap();
    let ant_names: Vec<&str> = ant_val["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    assert_eq!(ant_names, expected);

    let gemini = GeminiClient::new("k", "gemini-2.5-pro");
    let gem_val = serde_json::to_value(gemini.build_request_body(&req).unwrap().1).unwrap();
    let gem_names: Vec<&str> = gem_val["tools"][0]["functionDeclarations"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    assert_eq!(gem_names, expected);
}

#[test]
fn test_openai_multi_turn_prefix_stability() {
    let (t1, t2, t3, t4) = multi_turn_histories();
    let client = OpenAIClient::new("k", "gpt-4o");
    let tools = sample_tools();
    let cache = PromptCacheConfig::enabled()
        .with_key("agent-conv-1")
        .with_ttl(CacheTtl::TwentyFourHours);

    let turns: Vec<Value> = [t1, t2, t3, t4]
        .into_iter()
        .map(|msgs| {
            let req = ChatRequest::new(msgs)
                .with_system("Stable system prompt")
                .with_tools(tools.clone())
                .with_prompt_cache(cache.clone());
            serde_json::to_value(client.build_request_body(&req, true).unwrap()).unwrap()
        })
        .collect();

    for i in 0..turns.len() - 1 {
        assert_eq!(
            serde_json::to_string(&turns[i]["tools"]).unwrap(),
            serde_json::to_string(&turns[i + 1]["tools"]).unwrap(),
            "OpenAI tools must stay byte-identical across turns"
        );
        assert_eq!(turns[i]["prompt_cache_key"], "agent-conv-1");
        assert_eq!(turns[i]["prompt_cache_retention"], "24h");
        assert_json_array_is_prefix(
            &turns[i]["messages"],
            &turns[i + 1]["messages"],
            &format!("OpenAI turn {} -> {}", i + 1, i + 2),
        );
    }
}

#[test]
fn test_openai_responses_multi_turn_prefix_stability() {
    let (t1, t2, t3, t4) = multi_turn_histories();
    let client = OpenAIResponsesClient::new("k", "gpt-4.1");
    let tools = sample_tools();
    let cache = PromptCacheConfig::enabled().with_key("resp-conv-1");

    let turns: Vec<Value> = [t1, t2, t3, t4]
        .into_iter()
        .map(|msgs| {
            let req = ChatRequest::new(msgs)
                .with_system("Stable instructions")
                .with_tools(tools.clone())
                .with_prompt_cache(cache.clone());
            serde_json::to_value(client.build_request_body(&req, true).unwrap()).unwrap()
        })
        .collect();

    for i in 0..turns.len() - 1 {
        assert_eq!(turns[i]["instructions"], turns[i + 1]["instructions"]);
        assert_eq!(
            serde_json::to_string(&turns[i]["tools"]).unwrap(),
            serde_json::to_string(&turns[i + 1]["tools"]).unwrap()
        );
        assert_eq!(turns[i]["prompt_cache_key"], "resp-conv-1");
        assert!(turns[i].get("prompt_cache_retention").is_none());
        assert_json_array_is_prefix(
            &turns[i]["input"],
            &turns[i + 1]["input"],
            &format!("OpenAI Responses turn {} -> {}", i + 1, i + 2),
        );
    }
}

#[test]
fn test_anthropic_multi_turn_prefix_stability_and_cache_breakpoints() {
    let (t1, t2, t3, t4) = multi_turn_histories();
    let client = AnthropicClient::new("k", "claude-sonnet-4-20250514");
    let tools = sample_tools();
    let cache = PromptCacheConfig::enabled().with_ttl(CacheTtl::OneHour);

    let turns: Vec<Value> = [t1, t2, t3, t4]
        .into_iter()
        .map(|msgs| {
            let req = ChatRequest::new(msgs)
                .with_system("Stable Claude system prompt")
                .with_tools(tools.clone())
                .with_prompt_cache(cache.clone());
            serde_json::to_value(client.build_request_body(&req, true).unwrap()).unwrap()
        })
        .collect();

    for (idx, turn) in turns.iter().enumerate() {
        // Anthropic enforces a hard limit of 4 cache_control blocks per request.
        let cc_count = count_cache_control(turn);
        assert!(
            cc_count <= 4,
            "Turn {} exceeded Anthropic's 4-breakpoint limit: {}",
            idx + 1,
            cc_count
        );
        assert_eq!(
            cc_count, 3,
            "Expected 3 breakpoints (last tool, system, latest message tail)"
        );

        // Only the last tool has cache_control with ttl: "1h".
        let tool_arr = turn["tools"].as_array().unwrap();
        assert!(tool_arr[0].get("cache_control").is_none());
        assert!(tool_arr[1].get("cache_control").is_none());
        assert_eq!(
            tool_arr[2]["cache_control"],
            json!({"type": "ephemeral", "ttl": "1h"})
        );

        // System prompt has cache_control.
        assert_eq!(
            turn["system"][0]["cache_control"],
            json!({"type": "ephemeral", "ttl": "1h"})
        );

        // Only the last content block of the last message has cache_control.
        let msgs = turn["messages"].as_array().unwrap();
        for earlier_msg in &msgs[..msgs.len() - 1] {
            assert_eq!(
                count_cache_control(earlier_msg),
                0,
                "Earlier messages must not retain stale cache_control blocks"
            );
        }
        let last_blocks = msgs.last().unwrap()["content"].as_array().unwrap();
        assert_eq!(
            last_blocks.last().unwrap()["cache_control"],
            json!({"type": "ephemeral", "ttl": "1h"})
        );
    }

    // Verify prefix stability across turns: tools & system byte-identical,
    // and messages (ignoring the moving tail cache_control marker) strictly append-only.
    for i in 0..turns.len() - 1 {
        assert_eq!(
            serde_json::to_string(&turns[i]["tools"]).unwrap(),
            serde_json::to_string(&turns[i + 1]["tools"]).unwrap(),
            "Anthropic tools must stay byte-identical"
        );
        assert_eq!(
            serde_json::to_string(&turns[i]["system"]).unwrap(),
            serde_json::to_string(&turns[i + 1]["system"]).unwrap(),
            "Anthropic system must stay byte-identical"
        );
        let earlier_msgs = strip_cache_control(&turns[i]["messages"]);
        let later_msgs = strip_cache_control(&turns[i + 1]["messages"]);
        assert_json_array_is_prefix(
            &earlier_msgs,
            &later_msgs,
            &format!("Anthropic turn {} -> {}", i + 1, i + 2),
        );
    }
}

#[test]
fn test_anthropic_default_five_minute_ttl_omits_ttl_field() {
    let client = AnthropicClient::new("k", "claude-sonnet-4-20250514");
    let req = ChatRequest::new(vec![Message::user("hi")])
        .with_system("sys")
        .with_prompt_cache(PromptCacheConfig::enabled().with_ttl(CacheTtl::FiveMinutes));

    let body = serde_json::to_value(client.build_request_body(&req, false).unwrap()).unwrap();
    assert_eq!(body["system"][0]["cache_control"], json!({"type": "ephemeral"}));
    assert_eq!(
        body["messages"][0]["content"][0]["cache_control"],
        json!({"type": "ephemeral"})
    );
}

#[test]
fn test_gemini_multi_turn_implicit_prefix_stability_and_explicit_cached_content() {
    let (t1, t2, t3, t4) = multi_turn_histories();
    let client = GeminiClient::new("k", "gemini-2.5-pro");
    let tools = sample_tools();

    let turns: Vec<Value> = [t1, t2, t3, t4]
        .into_iter()
        .map(|msgs| {
            let req = ChatRequest::new(msgs)
                .with_system("Stable Gemini system instruction")
                .with_tools(tools.clone());
            serde_json::to_value(client.build_request_body(&req).unwrap().1).unwrap()
        })
        .collect();

    for i in 0..turns.len() - 1 {
        assert_eq!(
            serde_json::to_string(&turns[i]["systemInstruction"]).unwrap(),
            serde_json::to_string(&turns[i + 1]["systemInstruction"]).unwrap(),
            "Gemini systemInstruction must stay byte-identical"
        );
        assert_eq!(
            serde_json::to_string(&turns[i]["tools"]).unwrap(),
            serde_json::to_string(&turns[i + 1]["tools"]).unwrap(),
            "Gemini tools must stay byte-identical"
        );
        assert_json_array_is_prefix(
            &turns[i]["contents"],
            &turns[i + 1]["contents"],
            &format!("Gemini turn {} -> {}", i + 1, i + 2),
        );
    }

    // Verify thoughtSignature is preserved in Turn 2+ model content for Gemini thinking cache/continuity.
    assert_eq!(
        turns[1]["contents"][1]["parts"][0]["thoughtSignature"],
        "sig_turn2"
    );

    // Verify explicit cachedContent resource name forwarding.
    let explicit_req = ChatRequest::new(vec![Message::user("Use cache")])
        .with_prompt_cache(PromptCacheConfig::enabled().with_cached_content("cachedContents/abc123"));
    let explicit_val =
        serde_json::to_value(client.build_request_body(&explicit_req).unwrap().1).unwrap();
    assert_eq!(explicit_val["cachedContent"], "cachedContents/abc123");
}

#[test]
fn test_sampling_param_changes_do_not_alter_cached_prefix() {
    let (_, _, turn3, _) = multi_turn_histories();
    let base_req = ChatRequest::new(turn3)
        .with_system("System prompt")
        .with_tools(sample_tools())
        .with_prompt_cache(PromptCacheConfig::enabled());

    let modified_req = base_req
        .clone()
        .with_temperature(0.2)
        .with_max_tokens(2048)
        .with_top_p(0.9);

    let openai = OpenAIClient::new("k", "gpt-4o");
    let oa_a = serde_json::to_value(openai.build_request_body(&base_req, false).unwrap()).unwrap();
    let oa_b =
        serde_json::to_value(openai.build_request_body(&modified_req, false).unwrap()).unwrap();
    assert_eq!(oa_a["tools"], oa_b["tools"]);
    assert_eq!(oa_a["messages"], oa_b["messages"]);

    let anthropic = AnthropicClient::new("k", "claude-sonnet-4-20250514");
    let ant_a =
        serde_json::to_value(anthropic.build_request_body(&base_req, false).unwrap()).unwrap();
    let ant_b =
        serde_json::to_value(anthropic.build_request_body(&modified_req, false).unwrap()).unwrap();
    assert_eq!(ant_a["tools"], ant_b["tools"]);
    assert_eq!(ant_a["system"], ant_b["system"]);
    assert_eq!(ant_a["messages"], ant_b["messages"]);

    let gemini = GeminiClient::new("k", "gemini-2.5-pro");
    let gem_a = serde_json::to_value(gemini.build_request_body(&base_req).unwrap().1).unwrap();
    let gem_b = serde_json::to_value(gemini.build_request_body(&modified_req).unwrap().1).unwrap();
    assert_eq!(gem_a["systemInstruction"], gem_b["systemInstruction"]);
    assert_eq!(gem_a["tools"], gem_b["tools"]);
    assert_eq!(gem_a["contents"], gem_b["contents"]);
}

#[test]
fn test_reordering_tools_or_mutating_history_is_detected_as_prefix_change() {
    let tools_original = sample_tools();
    let mut tools_reordered = sample_tools();
    tools_reordered.swap(0, 1);

    let req1 = ChatRequest::new(vec![Message::user("hello")]).with_tools(tools_original);
    let req2 = ChatRequest::new(vec![Message::user("hello")]).with_tools(tools_reordered);

    let openai = OpenAIClient::new("k", "gpt-4o");
    let b1 = serde_json::to_string(&openai.build_request_body(&req1, false).unwrap()).unwrap();
    let b2 = serde_json::to_string(&openai.build_request_body(&req2, false).unwrap()).unwrap();
    assert_ne!(
        b1, b2,
        "Swapping tool order must be observable in serialized prefix"
    );

    let anthropic = AnthropicClient::new("k", "claude-sonnet-4-20250514");
    let a1 = serde_json::to_string(&anthropic.build_request_body(&req1, false).unwrap()).unwrap();
    let a2 = serde_json::to_string(&anthropic.build_request_body(&req2, false).unwrap()).unwrap();
    assert_ne!(a1, a2);

    let gemini = GeminiClient::new("k", "gemini-2.5-pro");
    let g1 = serde_json::to_string(&gemini.build_request_body(&req1).unwrap().1).unwrap();
    let g2 = serde_json::to_string(&gemini.build_request_body(&req2).unwrap().1).unwrap();
    assert_ne!(g1, g2);
}

#[test]
fn test_into_assistant_message_agent_loop_roundtrip_preserves_prefix() {
    use crate::{ChatResponse, StopReason};

    let tools = sample_tools();
    let mut history = vec![Message::user("Start task")];

    let req_t1 = ChatRequest::new(history.clone())
        .with_system("Agent system")
        .with_tools(tools.clone())
        .with_prompt_cache(PromptCacheConfig::enabled().with_key("loop-1"));

    // Simulate assistant response from Turn 1
    let resp_t1 = ChatResponse {
        id: Some("r1".to_string()),
        model: Some("model".to_string()),
        content: "Checking file".to_string(),
        thinking: Some("Need to inspect file first".to_string()),
        thinking_signature: Some("sig_loop_1".to_string()),
        tool_calls: vec![ToolCall::new(
            "call_loop_1",
            "zebra_read_file",
            json!({"path": "README.md", "offset": 0}),
        )],
        stop_reason: Some(StopReason::ToolUse),
        usage: None,
    };

    history.push(resp_t1.into_assistant_message());
    history.push(Message::tool_result(
        "call_loop_1",
        None::<String>, // Test Gemini name inference from prior ToolCall!
        "# oort",
        false,
    ));

    let req_t2 = ChatRequest::new(history.clone())
        .with_system("Agent system")
        .with_tools(tools.clone())
        .with_prompt_cache(PromptCacheConfig::enabled().with_key("loop-1"));

    let resp_t2 = ChatResponse {
        content: "Done reading README.".to_string(),
        stop_reason: Some(StopReason::EndTurn),
        ..Default::default()
    };
    history.push(resp_t2.into_assistant_message());
    history.push(Message::user("Summarize it"));

    let req_t3 = ChatRequest::new(history)
        .with_system("Agent system")
        .with_tools(tools)
        .with_prompt_cache(PromptCacheConfig::enabled().with_key("loop-1"));

    // Check OpenAI
    let oa = OpenAIClient::new("k", "gpt-4o");
    let oa1 = serde_json::to_value(oa.build_request_body(&req_t1, true).unwrap()).unwrap();
    let oa2 = serde_json::to_value(oa.build_request_body(&req_t2, true).unwrap()).unwrap();
    let oa3 = serde_json::to_value(oa.build_request_body(&req_t3, true).unwrap()).unwrap();
    assert_json_array_is_prefix(&oa1["messages"], &oa2["messages"], "OpenAI loop t1->t2");
    assert_json_array_is_prefix(&oa2["messages"], &oa3["messages"], "OpenAI loop t2->t3");

    // Check Anthropic
    let ant = AnthropicClient::new("k", "claude-sonnet-4-20250514");
    let a1 = serde_json::to_value(ant.build_request_body(&req_t1, true).unwrap()).unwrap();
    let a2 = serde_json::to_value(ant.build_request_body(&req_t2, true).unwrap()).unwrap();
    let a3 = serde_json::to_value(ant.build_request_body(&req_t3, true).unwrap()).unwrap();
    assert_json_array_is_prefix(
        &strip_cache_control(&a1["messages"]),
        &strip_cache_control(&a2["messages"]),
        "Anthropic loop t1->t2",
    );
    assert_json_array_is_prefix(
        &strip_cache_control(&a2["messages"]),
        &strip_cache_control(&a3["messages"]),
        "Anthropic loop t2->t3",
    );

    // Check Gemini (also verifies tool name was inferred from prior assistant ToolCall)
    let gem = GeminiClient::new("k", "gemini-2.5-pro");
    let g1 = serde_json::to_value(gem.build_request_body(&req_t1).unwrap().1).unwrap();
    let g2 = serde_json::to_value(gem.build_request_body(&req_t2).unwrap().1).unwrap();
    let g3 = serde_json::to_value(gem.build_request_body(&req_t3).unwrap().1).unwrap();
    assert_eq!(
        g2["contents"][2]["parts"][0]["functionResponse"]["name"],
        "zebra_read_file"
    );
    assert_json_array_is_prefix(&g1["contents"], &g2["contents"], "Gemini loop t1->t2");
    assert_json_array_is_prefix(&g2["contents"], &g3["contents"], "Gemini loop t2->t3");
}

#[test]
fn test_openai_legacy_completion_prefix_cache_stability() {
    use crate::CompletionRequest;

    let client = OpenAIClient::new("k", "gpt-3.5-turbo-instruct");
    let cache = PromptCacheConfig::enabled()
        .with_key("cmpl-session-1")
        .with_ttl(CacheTtl::TwentyFourHours);

    let req1 = CompletionRequest::new("fn add(a: i32, b: i32) -> i32 {\n")
        .with_prompt_cache(cache.clone());
    let req2 = CompletionRequest::new("fn add(a: i32, b: i32) -> i32 {\n    a + b\n}\n")
        .with_temperature(0.3)
        .with_max_tokens(128)
        .with_prompt_cache(cache);

    let v1a = serde_json::to_string(&client.build_completion_request_body(&req1, true).unwrap())
        .unwrap();
    let v1b = serde_json::to_string(&client.build_completion_request_body(&req1, true).unwrap())
        .unwrap();
    assert_eq!(v1a, v1b);

    let j1 = serde_json::to_value(client.build_completion_request_body(&req1, true).unwrap())
        .unwrap();
    let j2 = serde_json::to_value(client.build_completion_request_body(&req2, true).unwrap())
        .unwrap();

    assert_eq!(j1["prompt_cache_key"], "cmpl-session-1");
    assert_eq!(j1["prompt_cache_retention"], "24h");
    assert_eq!(j2["prompt_cache_key"], "cmpl-session-1");
    assert!(
        j2["prompt"]
            .as_str()
            .unwrap()
            .starts_with(j1["prompt"].as_str().unwrap()),
        "Extended completion prompt must preserve exact string prefix"
    );
}

#[test]
fn test_multimodal_image_and_pdf_encoding_and_prefix_stability() {
    let t1_msgs = vec![Message::user_parts(vec![
        ContentPart::text("Analyze this chart and PDF report."),
        ContentPart::image_base64("image/png", "iVBORw0KGgo="),
        ContentPart::image_url("https://example.com/chart.jpg"),
        ContentPart::pdf_base64("JVBERi0xLjQK", Some("report.pdf")),
        ContentPart::pdf_url("https://example.com/spec.pdf"),
        ContentPart::document_file_id("file-abc123", Some("application/pdf")),
    ])];

    let mut t2_msgs = t1_msgs.clone();
    t2_msgs.push(Message::assistant("The report shows 20% growth."));
    t2_msgs.push(Message::user("Extract the Q4 table from page 3."));

    let req1 = ChatRequest::new(t1_msgs)
        .with_system("Multimodal analyst")
        .with_prompt_cache(PromptCacheConfig::enabled());
    let req2 = ChatRequest::new(t2_msgs)
        .with_system("Multimodal analyst")
        .with_prompt_cache(PromptCacheConfig::enabled());

    // OpenAI Chat Completions
    let oa = OpenAIClient::new("k", "gpt-4o");
    let oa1 = serde_json::to_value(oa.build_request_body(&req1, false).unwrap()).unwrap();
    let oa2 = serde_json::to_value(oa.build_request_body(&req2, false).unwrap()).unwrap();
    let oa_parts = oa1["messages"][1]["content"].as_array().unwrap();
    assert_eq!(oa_parts[1]["type"], "image_url");
    assert_eq!(oa_parts[1]["image_url"]["url"], "data:image/png;base64,iVBORw0KGgo=");
    assert_eq!(oa_parts[2]["image_url"]["url"], "https://example.com/chart.jpg");
    assert_eq!(oa_parts[3]["type"], "file");
    assert_eq!(oa_parts[3]["file"]["filename"], "report.pdf");
    assert_eq!(
        oa_parts[3]["file"]["file_data"],
        "data:application/pdf;base64,JVBERi0xLjQK"
    );
    assert_eq!(oa_parts[5]["file"]["file_id"], "file-abc123");
    assert_json_array_is_prefix(&oa1["messages"], &oa2["messages"], "OpenAI multimodal t1->t2");

    // OpenAI Responses API
    let resp = OpenAIResponsesClient::new("k", "gpt-4.1");
    let r1 = serde_json::to_value(resp.build_request_body(&req1, false).unwrap()).unwrap();
    let r2 = serde_json::to_value(resp.build_request_body(&req2, false).unwrap()).unwrap();
    let r_parts = r1["input"][0]["content"].as_array().unwrap();
    assert_eq!(r_parts[1]["type"], "input_image");
    assert_eq!(r_parts[3]["type"], "input_file");
    assert_eq!(r_parts[3]["filename"], "report.pdf");
    assert_eq!(
        r_parts[3]["file_data"],
        "data:application/pdf;base64,JVBERi0xLjQK"
    );
    assert_eq!(r_parts[4]["file_url"], "https://example.com/spec.pdf");
    assert_eq!(r_parts[5]["file_id"], "file-abc123");
    assert_json_array_is_prefix(&r1["input"], &r2["input"], "OpenAI Responses multimodal t1->t2");

    // Anthropic Messages API
    let ant = AnthropicClient::new("k", "claude-sonnet-4-20250514");
    let a1 = serde_json::to_value(ant.build_request_body(&req1, false).unwrap()).unwrap();
    let a2 = serde_json::to_value(ant.build_request_body(&req2, false).unwrap()).unwrap();
    let a_parts = a1["messages"][0]["content"].as_array().unwrap();
    assert_eq!(a_parts[1]["type"], "image");
    assert_eq!(a_parts[1]["source"]["type"], "base64");
    assert_eq!(a_parts[2]["source"]["type"], "url");
    assert_eq!(a_parts[3]["type"], "document");
    assert_eq!(a_parts[3]["title"], "report.pdf");
    assert_eq!(a_parts[3]["source"]["media_type"], "application/pdf");
    assert_eq!(a_parts[3]["source"]["data"], "JVBERi0xLjQK");
    assert_eq!(a_parts[4]["source"]["type"], "url");
    assert_eq!(a_parts[5]["source"]["type"], "file");
    assert_eq!(a_parts[5]["source"]["file_id"], "file-abc123");
    assert_json_array_is_prefix(
        &strip_cache_control(&a1["messages"]),
        &strip_cache_control(&a2["messages"]),
        "Anthropic multimodal t1->t2",
    );

    // Gemini API
    let gem = GeminiClient::new("k", "gemini-2.5-pro");
    let g1 = serde_json::to_value(gem.build_request_body(&req1).unwrap().1).unwrap();
    let g2 = serde_json::to_value(gem.build_request_body(&req2).unwrap().1).unwrap();
    let g_parts = g1["contents"][0]["parts"].as_array().unwrap();
    assert_eq!(g_parts[1]["inlineData"]["mimeType"], "image/png");
    assert_eq!(g_parts[2]["fileData"]["fileUri"], "https://example.com/chart.jpg");
    assert_eq!(g_parts[3]["inlineData"]["mimeType"], "application/pdf");
    assert_eq!(g_parts[3]["inlineData"]["data"], "JVBERi0xLjQK");
    assert_eq!(g_parts[4]["fileData"]["mimeType"], "application/pdf");
    assert_eq!(g_parts[4]["fileData"]["fileUri"], "https://example.com/spec.pdf");
    assert_eq!(g_parts[5]["fileData"]["fileUri"], "file-abc123");
    assert_json_array_is_prefix(&g1["contents"], &g2["contents"], "Gemini multimodal t1->t2");
}
