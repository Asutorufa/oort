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

- [`crates/storage`](crates/storage) (`oort-storage`): SQLite (`sqlx`) `SessionStore`:
  - `sessions` / append-only `messages` / `compactions` tables; triggers enforce append-only logs, contiguous seq and an immutable `system_prompt` / `frozen_tools` / `config_json` prefix
  - One transaction per agent step (`PersistenceHook`), WAL + foreign keys; byte-identical prefix after resume
- [`crates/mcp`](crates/mcp) (`oort-mcp`): lightweight MCP client as a `ToolExecutor`:
  - stdio and Streamable HTTP transports (custom headers, pluggable bearer/OAuth `TokenProvider`)
  - Frozen, name-sorted catalogs with deterministic schemas; `readOnlyHint` → concurrent execution
  - `CompositeToolExecutor` (in `oort-agent`) combines servers and local tools with `{server}__{tool}` namespaces
- [`crates/server`](crates/server) (`oort-server`) + [`web/`](web): debug console (axum + SSE backend, React/Vite frontend)

## Debug console

The console shows streaming output, thinking and tool blocks, a human-approval card, Send/Steer vs Interrupt vs Compact controls, and a debug panel (state machine phase, per-turn tokens, prefix-cache hit rate, context usage, compaction points, raw history vs active view, event log).

### Development (two terminals)

```bash
cargo run -p oort-server
```

```bash
cd web && npm install && npm run dev
```

Open the URL Vite prints (default http://localhost:5173). Vite proxies `/api` to `http://127.0.0.1:3000` (override with `OORT_BACKEND`).

The `mock` provider works without API keys (keywords: `time`, `list`, `read <path>`, `note <text>`, `think`, `slow`, or any tool name). For real providers, set `ANTHROPIC_API_KEY`, `OPENAI_API_KEY` or `GEMINI_API_KEY` (optional `*_BASE_URL`) before starting the server.

### Single process (built frontend)

```bash
cd web && npm run build && cd .. && OORT_WEB_DIST=web/dist cargo run -p oort-server
```

### Server configuration (environment variables)

| Variable | Default | Meaning |
|---|---|---|
| `OORT_ADDR` | `127.0.0.1:3000` | Listen address |
| `OORT_DB` | `sqlite://oort.db` | Session database |
| `OORT_MCP_CONFIG` | — | JSON file `{"mcpServers": {"name": {"command": "...", "args": []} \| {"url": "...", "headers": {}, "bearerToken": "..."}}}` |
| `OORT_WORKSPACE` | `.` | Root directory for the built-in `list_dir` / `read_file` / `write_note` tools |
| `OORT_WEB_DIST` | — | Serve the built frontend from this directory |

Try MCP with the bundled mock server: build it with `cargo build -p oort-mcp`, then point `OORT_MCP_CONFIG` at `{"mcpServers": {"mock": {"command": "target/debug/oort-mcp-mock"}}}`.

The tool catalog is frozen at startup. A stored session can only resume with the same catalog (otherwise the provider prefix cache would break), so changing MCP servers makes older sessions fail with a catalog-mismatch error.

### API

| Endpoint | Purpose |
|---|---|
| `POST /api/sessions` | Create (`provider`, `model`, `system_prompt`, `max_context_tokens`, `approval`, `message`) |
| `GET /api/sessions` | List sessions |
| `GET /api/sessions/:id` | Snapshot: raw history, active view, checkpoints, usage, pending approvals |
| `GET /api/sessions/:id/events` | SSE stream of `{"kind":"agent","seq":n,"event":AgentEvent}` / `loop_exited` / `lagged` |
| `POST /api/sessions/:id/input` | `{"type":"steer"\|"interrupt","text"}`, `{"type":"approval","call_id","decision":"approve"\|"deny","reason"?}`, `{"type":"compact"}` |
| `POST /api/sessions/:id/cancel` | Stop the background loop (state is kept and can be resumed) |

## Testing

```bash
cargo test --workspace
(cd web && npm run typecheck)
```
