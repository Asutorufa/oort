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
  - MCP OAuth 2.0: RFC 9728/8414 discovery, Dynamic Client Registration, PKCE `S256`, resource indicators, automatic refresh
  - Frozen, name-sorted catalogs with deterministic schemas; `readOnlyHint` → concurrent execution
  - `CompositeToolExecutor` (in `oort-agent`) combines servers and local tools with `{server}__{tool}` namespaces
- [`crates/skills`](crates/skills) (`oort-skills`): `SKILL.md` parsing and a `SkillRegistry` over skill directories and the database
- [`crates/server`](crates/server) (`oort-server`) + [`web/`](web): Oort app (axum + SSE backend, React/Vite frontend)

## Oort app

`web/` (React + Vite) is a consumer-style app:

- **Chat**: a centered thread with Markdown (highlighted, copyable code), Intelligent UI cards (charts, tables, forms, approval diffs, action chips), and one quiet activity line per turn (「思考して 2 件のツールを実行 (0.22s) ⌄」) that expands into thinking and tool details. The floating composer completes `/` skills and commands, switches the approval policy, and while the agent works offers *キューに追記* (Steer) and *中断して送信* (Interrupt). The **透視** drawer holds the internals: prefix-cache hit rate and breakdown, TTFT and total time per call, manual compaction, raw vs active history, and the event log. The event stream resumes after reloads and dropped connections.
- **Sidebar** (collapsible): new chat, conversations (running / waiting-for-approval indicators, rename, export JSON, delete), navigation, theme dots (星雲暮紫 `iris` / 月光極簡 `lunar`) and light/dark, remembered in `localStorage`.
- **ツール & MCP**: servers, connection test and tool discovery, the Tool Playground, and OAuth 2.0 (authorize in a popup, scopes and expiry, reconnect / disconnect).
- **Skills**: `SKILL.md` files and app-made skills, search, preview and editor, rescan.
- **モデル設定** (providers) and **Presets** (provider, model, prompt, MCP servers, skills, approval policy).

### Development (two terminals)

```bash
cargo run -p oort-server
```

```bash
cd web && npm install && npm run dev
```

Open the URL Vite prints (default http://localhost:5173). Vite proxies `/api` to `http://127.0.0.1:3000` (override with `OORT_BACKEND`).

LLM providers, MCP servers and presets are managed in the workbench (stored in the same SQLite database). A `Mock (offline)` provider is built in and needs no key (keywords: `time`, `list`, `read <path>`, `note <text>`, `think`, `slow`, or any tool name).

API keys, header values, stdio `env` values and bearer tokens may be written as `env:NAME`: they are resolved from the server's environment when used, so the secret never has to be stored. Literal secrets are stored as given and always returned masked (`AIza…9x2Q`); sending back an empty or masked value keeps the stored one.

### Single process (built frontend)

```bash
cd web && npm run build && cd .. && OORT_WEB_DIST=web/dist cargo run -p oort-server
```

### Server configuration (environment variables)

| Variable | Default | Meaning |
|---|---|---|
| `OORT_ADDR` | `127.0.0.1:3000` | Listen address |
| `OORT_DB` | `sqlite://oort.db` | Database (sessions and configuration) |
| `OORT_MCP_CONFIG` | — | Optional `{"mcpServers": {"name": {"command": "...", "args": []} \| {"url": "...", "headers": {}, "bearerToken": "..."}}}` file, imported into the MCP server list at startup (existing names are left alone) |
| `OORT_WORKSPACE` | `.` | Root directory for the built-in `list_dir` / `read_file` / `write_note` tools |
| `OORT_WEB_DIST` | — | Serve the built frontend from this directory |
| `OORT_PUBLIC_URL` | `http://{OORT_ADDR}` | Base URL browsers use to reach the server; the MCP OAuth redirect URI is `{OORT_PUBLIC_URL}/api/mcp/oauth/callback` |
| `OORT_SKILLS_DIRS` | `~/.oort/skills:{workspace}/skills` | Directories scanned for `<skill>/SKILL.md` |

Try MCP with the bundled mock server: `cargo build -p oort-mcp`, then add a `stdio` server with command `target/debug/oort-mcp-mock` and press *Test & Discover*.

Sessions created before the workbench keep working with `*_API_KEY` / `*_BASE_URL` environment variables.

### Frozen tools and configuration changes

A session's system prompt and tool catalog are frozen (sorted) when it is created, and never derived again, so the provider prefix cache stays valid. Editing, renaming or deleting an MCP server only affects new sessions: an existing session always sends its frozen catalog, served by whatever connection is live; a tool that no longer exists answers with an error result. Deleting a provider makes its sessions read-only (they cannot run again).

### MCP OAuth 2.0

Set a Streamable HTTP server's auth to `oauth` (optionally with `client_id`, `client_secret` and `scopes`) and press *OAuth で認証*. The server follows the MCP Authorization spec: the `401`'s `WWW-Authenticate` points to the protected-resource metadata (RFC 9728), which names the authorization server (RFC 8414); the client is registered dynamically (RFC 7591) or the configured one is used; the code flow uses PKCE `S256` and the `resource` indicator (RFC 8707). Tokens live in `mcp_oauth_tokens`, are never returned by the API, and are refreshed shortly before expiry and after a `401`; if refreshing fails the server shows `auth_required`.

### Skills

A skill is a `SKILL.md` with YAML frontmatter (`name`, `description`, optional `allowed_tools`, `tags`) and Markdown instructions, read from the skill directories or created in the UI (stored in SQLite; a database skill shadows a file skill with the same name). A session freezes only the sorted `name: description` index of its skills into the system prompt; the model fetches a body with the read-only `load_skill` tool, and the user can type `/<skill> …` to append it to their message. Bodies therefore never enter the cached prefix, and editing a skill never rewrites a session.

### Intelligent UI widgets

New sessions get a read-only `render_widget` tool. Its arguments *are* the widget (`metrics_chart`, `data_table`, `interactive_form`, `approval_diff`, `action_chips`), so widgets are stored in the append-only history like any tool call. Interactions come back as ordinary user messages (`[widget_response id=<call id> …]` + JSON); `approval_diff` always waits for the approval buttons, whatever the approval policy.

### API

| Endpoint | Purpose |
|---|---|
| `GET / POST /api/providers`, `GET / PUT / DELETE /api/providers/:id` | Provider CRUD (`kind`: `openai` = Responses API, `openai_compatible` = Chat Completions with `base_url`, `anthropic`, `gemini`, `mock`) |
| `POST /api/providers/:id/test`, `POST /api/providers/test` | Connectivity test (`list_models`; `{"ping": true}` adds a 1-token chat). Body may carry unsaved changes. → `{ok, latency_ms, model_checked, model_found, models, error}` |
| `GET / POST /api/mcp-servers`, `GET / PUT / DELETE /api/mcp-servers/:id` | MCP server CRUD (`streamable_http` with `url`/`headers`/`auth`, or `stdio` with `command`/`args`/`env`) |
| `POST /api/mcp-servers/:id/test`, `POST /api/mcp-servers/test` | Fresh connection: `initialize` + `tools/list` → `{ok, latency_ms, server_info, tools: [{name, description, read_only, input_schema}], error}` |
| `POST /api/mcp-servers/:id/tools/call` | Tool playground, no LLM: `{tool_name, arguments, timeout_ms?}` → `{ok, latency_ms, is_error, content, structured_content, text, error}` |
| `GET / POST /api/presets`, `GET / PUT / DELETE /api/presets/:id` | Agent templates (provider, model, system prompt, MCP servers, skills, built-in tools, approval policy) |
| `POST /api/mcp-servers/:id/oauth/start` | Discovery + client registration + PKCE → `{authorization_url, state}` (open it in a popup) |
| `GET /api/mcp/oauth/callback` | Authorization redirect target: validates `state`, exchanges the code, posts `{type: "oort-oauth", server_id, ok}` to the opener |
| `GET /api/mcp-servers/:id/oauth`, `POST /api/mcp-servers/:id/oauth/disconnect` | Connection status (no secrets) / forget the tokens |
| `GET / POST /api/skills`, `GET / PUT / DELETE /api/skills/:id`, `POST /api/skills/reload` | Skills (file skills `file:<name>` are read-only) and directory rescan |
| `POST /api/sessions` | Create from `preset_id` and/or `provider_id`, `model`, `mcp_server_ids`, `skill_ids`, `widgets`, `builtin_tools`, `system_prompt`, `approval_policy` (`auto_all` / `auto_readonly` / `always_ask`), `max_context_tokens`, `max_tokens`, `title`, `message` |
| `GET /api/sessions` | List sessions |
| `GET /api/sessions/:id` | Snapshot: raw history, active view, checkpoints, pending approvals, usage with per-call `ttft_ms`, `latency_ms` and a request summary (sizes and prefix hashes) |
| `PATCH /api/sessions/:id` | `{title?, approval_policy?}` (applies to a running loop immediately) |
| `DELETE /api/sessions/:id` | Stop the loop and delete the session with its history |
| `GET /api/sessions/:id/export` | Debug JSON: metadata, settings, frozen prompt and tools, raw history, compactions, LLM call metrics (no secrets) |
| `GET /api/sessions/:id/events` | SSE: `hello` (status + in-progress turn), then `agent` / `widget` / `loop_exited` / `session_updated` / `deleted` / `lagged`. Ids are `{boot}:{seq}`; reconnecting with `Last-Event-ID` replays missed events (`hello.resumed`), otherwise the client refetches the snapshot |
| `POST /api/sessions/:id/input` | `{"type":"steer"\|"interrupt","text"}`, `{"type":"approval","call_id","decision":"approve"\|"deny","reason"?}`, `{"type":"compact"}` |
| `POST /api/sessions/:id/cancel` | Stop the background loop (state is kept and can be resumed) |

## Testing

```bash
cargo test --workspace
(cd web && npm run typecheck)
```
