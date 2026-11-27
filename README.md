# oort

A Rust LLM agent workspace.

## Crates

- [`crates/llm`](crates/llm) (`oort-llm`): Pure-HTTP LLM API client crate (no third-party LLM SDKs) supporting:
  - **OpenAI Chat Completions & Legacy Completions** (`/v1/chat/completions`, `/v1/completions`, `/v1/models`) & OpenAI-compatible APIs
  - **OpenAI Responses API** (`/v1/responses`, `/v1/models`)
  - **Anthropic Messages API** (`/v1/messages`, `/v1/models`)
  - **Google Gemini API** (Google AI Studio & Vertex AI: `generateContent`, `streamGenerateContent`, `models`)
  - SSE streaming with UTF-8 chunk-boundary safety and `StreamCollector` / `collect_stream`
  - Tool / function calling, extended thinking / reasoning tokens & signatures, multimodal image and PDF/document inputs
  - Prompt prefix caching (`PromptCacheConfig`) with strict deterministic serialization

## Testing

```bash
cargo test --workspace
```
