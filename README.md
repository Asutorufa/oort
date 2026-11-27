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
  - Provider-agnostic `CacheBreakpoint`s (translated to Anthropic `cache_control`, ignored elsewhere)
- [`crates/agent`](crates/agent) (`oort-agent`): Provider-, UI- and runtime-agnostic agent loop engine:
  - State machine (`Agent::step` / `Agent::run_loop`) over a serializable, append-only `AgentState`
  - `ModelClient`, `Tool` / `ToolExecutor`, `ContextManager`, `Compactor`, `LoopHook` trait boundaries (`Arc<dyn Trait>`)
  - Read-only tools run concurrently (`JoinSet`), side-effecting tools sequentially
  - `LoopInput::{Steer, Interrupt, Approval, Compact, Shutdown}` inbox and `AgentEvent` outbox over `mpsc`
  - Two-level cancellation (session / turn) with synthetic `ToolResult`s on interrupt
  - Human-in-the-loop approval with pause & resume from persisted state
  - Epoch-based compaction that never rewrites history and never splits tool-call pairs

## Testing

```bash
cargo test --workspace
```
