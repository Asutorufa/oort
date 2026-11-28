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
- [`crates/tools`](crates/tools) (`oort-tools`): builtin tool groups (self-config, plan/ask, memory, utilities, local workspace)
- [`crates/server`](crates/server) (`oort-server`) + [`web/`](web): Oort app (axum + SSE backend, React/Vite frontend)

## Oort app

`web/` (React + Vite) is a consumer-style app:

- **Chat**: a centered thread with Markdown (highlighted, copyable code), Intelligent UI cards (charts, tables, forms, approval diffs, action chips), and one quiet activity line per turn (e.g. "thought and ran 2 tools (0.22s)") that expands into thinking and tool details. The floating composer completes `/` skills and commands, switches the approval policy, and while the agent works offers *queue* (Steer) and *interrupt and send* (Interrupt). The **Inspector** drawer holds the internals: prefix-cache hit rate and breakdown, TTFT and total time per call, manual compaction, raw vs active history, and the event log. The event stream resumes after reloads and dropped connections.
- **Sidebar** (collapsible): new chat, conversation search and list (running / waiting-for-approval indicators, rename, export JSON, delete), and one button into settings.
- **Settings** (`⌘,` / `Ctrl+,`, `Esc` to close): a full-screen workspace over the chat, which stays mounted. General & appearance (language, light / dark / system, accent theme), then the pages below.
- **Tools & MCP**: servers, connection test and tool discovery, the Tool Playground, and OAuth 2.0 (authorize in a popup, scopes and expiry, reconnect / disconnect).
- **Skills**: `SKILL.md` files and app-made skills, search, preview and editor, rescan.
- **Model settings** (providers) and **Presets** (provider, model, prompt, MCP servers, skills, approval policy).

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
| `OORT_WEB_FETCH_ALLOW_PRIVATE` | — | `1` lets `web_fetch` reach loopback / private addresses (refused by default) |

Try MCP with the bundled mock server: `cargo build -p oort-mcp`, then add a `stdio` server with command `target/debug/oort-mcp-mock` and press *Test & Discover*.

Sessions created before the workbench keep working with `*_API_KEY` / `*_BASE_URL` environment variables.

### Session titles

A new chat is named automatically, without touching the conversation's requests (so the prefix cache is unaffected):

1. A `deepseek` provider is asked for a native title (`"title": {"enabled": true}`) with the first request only.
2. Otherwise (or when DeepSeek returns none), one small background request after the first turn: its own system prompt, no tools, thinking off, 64 output tokens, 15 s.
3. If that fails, the start of the first message.

A title the user types (or passes to `POST /api/sessions`) is never replaced (`title_source = 'manual'`), and subagent sessions are not named. The sidebar menu can regenerate a title. The offline mock provider goes straight to step 3.

### Frozen tools and configuration changes

A session's system prompt and tool catalog are frozen (sorted) when it is created, and never derived again, so the provider prefix cache stays valid. Editing, renaming or deleting an MCP server only affects new sessions: an existing session always sends its frozen catalog, served by whatever connection is live; a tool that no longer exists answers with an error result. Deleting a provider makes its sessions read-only (they cannot run again).

### MCP OAuth 2.0

Set a Streamable HTTP server's auth to `oauth` (optionally with `client_id`, `client_secret` and `scopes`) and press the OAuth connect button. The server follows the MCP Authorization spec: the `401`'s `WWW-Authenticate` points to the protected-resource metadata (RFC 9728), which names the authorization server (RFC 8414); the client is registered dynamically (RFC 7591) or the configured one is used; the code flow uses PKCE `S256` and the `resource` indicator (RFC 8707). Tokens live in `mcp_oauth_tokens`, are never returned by the API, and are refreshed shortly before expiry and after a `401`; if refreshing fails the server shows `auth_required`.

### Skills

A skill is a `SKILL.md` with YAML frontmatter (`name`, `description`, optional `allowed_tools`, `tags`) and Markdown instructions, read from the skill directories or created in the UI (stored in SQLite; a database skill shadows a file skill with the same name). A session freezes only the sorted `name: description` index of its skills into the system prompt; the model fetches a body with the read-only `load_skill` tool, and the user can type `/<skill> …` to append it to their message. Bodies therefore never enter the cached prefix, and editing a skill never rewrites a session.

### Intelligent UI widgets

New sessions get a read-only `render_widget` tool. Its arguments *are* the widget (`metrics_chart`, `data_table`, `interactive_form`, `approval_diff`, `action_chips`), so widgets are stored in the append-only history like any tool call. Interactions come back as ordinary user messages (`[widget_response id=<call id> …]` + JSON); `approval_diff` always waits for the approval buttons, whatever the approval policy.

### Builtin tools

Sessions (and presets) choose builtin tool groups at creation; their tools are frozen with the session's catalog like MCP tools. Domain tools stay in MCP.

| Group | Tools | Notes |
|---|---|---|
| `self_config` | `save_skill`, `configure_mcp_server` | always ask, even under *auto all*; new MCP servers apply to new sessions only |
| `core_ux` | `update_plan`, `ask_user` (+ `render_widget`) | `ask_user` pauses like an approval and its answer becomes the result (survives restarts) |
| `memory` | `memory_save`, `memory_search` | SQLite FTS5 with trigram tokens (works for Japanese/Chinese); searched on demand, never injected into prompts |
| `utilities` | `current_time`, `web_fetch` | `web_fetch` converts HTML to Markdown, trims, follows redirects and refuses private addresses |
| `subagent` | `spawn_subagent` | see below |
| `workspace` | `read_file`, `write_file`, `run_command` | only with a `workspace_root`; paths cannot escape it (`..`, symlinks); writes show a diff; `run_command` always asks, has a timeout and is killed with its process group on interrupt |

### Subagents

### Thinking level and provider defaults

Providers carry generation defaults: `defaults: {reasoning_effort, temperature, max_output_tokens, context_window, timeout_secs}`. `reasoning_effort` is provider-neutral (`default`, `off`, `low`, `medium`, `high`) and is resolved session → preset → provider. Each adapter maps it to its own parameter: OpenAI `reasoning_effort` (sampling parameters are dropped with it), Anthropic `thinking` with a 4k / 10k / 24k token budget (temperature and top_k dropped, `max_tokens` kept above the budget), Gemini `thinkingBudget` on 2.x models and `thinkingLevel` on later ones (Pro models cannot turn thinking off). It is a request parameter outside the cached prefix, so a chat can change it mid-conversation. `context_window` sets the compaction threshold of new sessions; `timeout_secs` is a per-read timeout.

### Web UI languages

The web UI is available in English, Japanese and Korean (`web/src/i18n`). The first visit follows the browser language; the choice is stored in `localStorage` (`oort.locale`) and sets `<html lang>`. The dictionaries are type-checked against each other, so a missing or extra key fails `tsc`. The UI language never changes prompts or history.

### Loop errors and automatic retry

When the loop stops with an error (an HTTP 429 quota error, a 5xx, a dropped connection, …) the session detail carries `last_error` (`message`, `at`, `retry_at`, `attempt`, `max_attempts`) and the session list marks it `failed`. The web UI shows the error in the conversation with a plain explanation, the provider's suggested wait and a retry button. A failed session is not resumed by reconnecting; it waits for `POST /api/sessions/:id/retry` or new input.

Each provider has an `auto_retry` setting (`{ "enabled": false, "max_attempts": 3 }` by default). When it is enabled, transient errors (408, 429, 5xx, network) are retried after the provider's hint (`Please retry in 13s`) plus a second, or after an exponential backoff (5s, 10s, … at most 2 minutes), up to `max_attempts` times. Authentication and other client errors are never retried. The offline mock model fails on purpose for a user message line `!error <status> [once] <message>`.

`spawn_subagent` runs an isolated child session (stored with `parent_session_id`, hidden from the session list, listed by `GET /api/sessions/:id/children`) and returns only its final report. Children are read-only: they get read-only tools, writes are refused with a message asking them to report proposed changes, they cannot ask the user, and they cannot spawn beyond `max_depth` (default 1). Several subagents in one turn run in parallel; interrupting or stopping the parent cancels them. A turn budget makes them summarize instead of failing, and a timeout returns partial findings. Profiles: `explorer`, `researcher`, `reviewer`, or `preset:<name>`. `context_mode: "fork"` copies the parent's history prefix and tool catalog byte for byte so the provider can reuse the parent's cached prefix. Child events are relayed to the parent's stream as `subagent` events.

### Commands and explicit skills

`/compact`, `/approval <auto|readonly|ask>`, `/stop`, `/export` and `/new [preset]` run without the model (`POST /api/commands/run`); `GET /api/commands` lists them together with user-invocable skills. Invoking a skill (`/name args` or `invoked_skills` on input) appends `<active_skill name="…">` with `$ARGUMENTS` substituted to that user message only; a skill already loaded in the current epoch is replaced by `<skill_already_loaded name="…" />` (the set is persisted and cleared by compaction). Skills with `execution_mode: subagent` run in a subagent and their report is sent back as a message.

### Usage and budget

Every LLM call (conversation turns, subagents, compaction and title requests) and every executed tool call is appended to its own ledger (`usage_ledger`, `tool_usage_ledger`). Deleting a chat keeps those rows (`session_id` becomes `NULL`). Recording only observes the streams: requests, history and the provider's prefix cache are unchanged. A stream that is interrupted before the provider reports usage is not counted.

Costs are estimates. A price is resolved from your override, then the built-in list-price table (`crates/storage/src/pricing.rs`, a 2026-11 snapshot), then `ollama` = free; unknown models count as $0 until you set a price. The budget only warns (a dot in the settings navigation); it never stops a chat. Usage recorded before this feature is copied into the ledger once at startup.

| Endpoint | Meaning |
|---|---|
| `GET /api/analytics/usage?range=7d\|30d\|month\|20w&tz_offset_minutes=&calendar_month=YYYY-MM` | Summary, budget status, daily series, per model / tool / chat breakdown |
| `GET / PUT /api/analytics/budget` | `{monthly_budget_usd, warning_threshold_pct}` |
| `GET / PUT / DELETE /api/analytics/pricing` | Custom USD prices per 1M tokens (`model_pattern`, a trailing `*` matches a prefix); changes re-price recorded calls unless `recalculate` is `false` |

### API

| Endpoint | Purpose |
|---|---|
| `GET / POST /api/providers`, `GET / PUT / DELETE /api/providers/:id` | Provider CRUD (`kind`: `openai` = Responses API, `openai_compatible` = Chat Completions with `base_url`, `anthropic`, `gemini`, `mock`) |
| `POST /api/providers/:id/models`, `POST /api/providers/models` | Model ids the provider offers (body may carry unsaved changes) → `{models}`; 502 with the reason when listing fails |
| `POST /api/providers/:id/test`, `POST /api/providers/test` | Connectivity test (`list_models`; `{"ping": true}` adds a 1-token chat). Body may carry unsaved changes. → `{ok, latency_ms, model_checked, model_found, models, error}` |
| `GET / POST /api/mcp-servers`, `GET / PUT / DELETE /api/mcp-servers/:id` | MCP server CRUD (`streamable_http` with `url`/`headers`/`auth`, or `stdio` with `command`/`args`/`env`) |
| `POST /api/mcp-servers/:id/test`, `POST /api/mcp-servers/test` | Fresh connection: `initialize` + `tools/list` → `{ok, latency_ms, server_info, tools: [{name, description, read_only, input_schema}], error}` |
| `POST /api/mcp-servers/:id/tools/call` | Tool playground, no LLM: `{tool_name, arguments, timeout_ms?}` → `{ok, latency_ms, is_error, content, structured_content, text, error}` |
| `GET / POST /api/presets`, `GET / PUT / DELETE /api/presets/:id` | Agent templates (provider, model, system prompt, MCP servers, skills, built-in tools, approval policy) |
| `POST /api/mcp-servers/:id/oauth/start` | Discovery + client registration + PKCE → `{authorization_url, state}` (open it in a popup) |
| `GET /api/mcp/oauth/callback` | Authorization redirect target: validates `state`, exchanges the code, posts `{type: "oort-oauth", server_id, ok}` to the opener |
| `GET /api/mcp-servers/:id/oauth`, `POST /api/mcp-servers/:id/oauth/disconnect` | Connection status (no secrets) / forget the tokens |
| `GET / POST /api/skills`, `GET / PUT / DELETE /api/skills/:id`, `POST /api/skills/reload` | Skills (file skills `file:<name>` are read-only) and directory rescan; `user_invocable`, `argument_hint`, `execution_mode`, `subagent_profile` |
| `GET /api/memories?q=&tag=&scope=`, `POST /api/memories`, `GET / PUT / DELETE /api/memories/:id` | Memories (full-text search) |
| `GET /api/commands`, `POST /api/commands/run` | Slash commands (`{session_id?, name, arguments?}` → `{message, effect: {navigate_session?, download?}}`) |
| `GET /api/sessions/:id/children` | Subagent sessions of a session |
| `POST /api/sessions` | Create from `preset_id` and/or `provider_id`, `model`, `mcp_server_ids`, `skill_ids`, `builtin_groups`, `workspace_root`, `widgets`, `builtin_tools`, `system_prompt`, `approval_policy` (`auto_all` / `auto_readonly` / `always_ask`), `max_context_tokens`, `max_tokens`, `title`, `message` |
| `GET /api/sessions` | List sessions |
| `GET /api/sessions/:id` | Snapshot: raw history, active view, checkpoints, pending approvals, usage with per-call `ttft_ms`, `latency_ms` and a request summary (sizes and prefix hashes) |
| `PATCH /api/sessions/:id` | `{title?, approval_policy?, reasoning_effort?, model?}` (applies to a running loop immediately; the thinking level and model change the next request only, never the history; `model` switches within the session's provider) |
| `POST /api/sessions/:id/title/regenerate` | Generate the title again (a rename made meanwhile wins) |
| `DELETE /api/sessions/:id` | Stop the loop and delete the session with its history |
| `GET /api/sessions/:id/export` | Debug JSON: metadata, settings, frozen prompt and tools, raw history, compactions, LLM call metrics (no secrets) |
| `GET /api/sessions/:id/events` | SSE: `hello` (status + in-progress turn), then `agent` (including `title_updated`) / `widget` / `loop_exited` / `session_updated` / `deleted` / `lagged`. Ids are `{boot}:{seq}`; reconnecting with `Last-Event-ID` replays missed events (`hello.resumed`), otherwise the client refetches the snapshot |
| `POST /api/sessions/:id/input` | `{"type":"steer"\|"interrupt","text","invoked_skills"?:[{name,arguments?}]}`, `{"type":"approval","call_id","decision":"approve"\|"deny"\|"answer","reason"?,"content"?}`, `{"type":"compact"}` |
| `POST /api/sessions/:id/cancel` | Stop the background loop (state is kept and can be resumed); also cancels a scheduled automatic retry |
| `POST /api/sessions/:id/retry` | Start the loop again after it stopped with an error (`last_error` in the session detail) |

## Testing

```bash
cargo test --workspace
(cd web && npm run typecheck)
```
