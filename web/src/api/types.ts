import type { Key } from "../i18n";
// Mirrors crates/server/src/{api,config_api}.rs and the serde shapes of oort-agent / oort-llm.

// ----- IR ------------------------------------------------------------------------------------

export type Role = "system" | "user" | "assistant" | "tool";

export interface ToolCall {
  id: string;
  name: string;
  arguments: unknown;
}

export interface ToolResult {
  tool_call_id: string;
  name?: string;
  content: string;
  is_error: boolean;
}

export type Vendor = "openai" | "anthropic" | "gemini" | "deepseek" | "openrouter" | "qwen" | "ollama" | "custom";

export interface WebSearchToolConfig {
  enabled: boolean;
  max_uses?: number | null;
  allowed_domains?: string[];
  blocked_domains?: string[];
}

export interface FileSearchToolConfig {
  enabled: boolean;
  vector_store_ids?: string[];
}

export interface ServerToolConfig {
  web_search?: WebSearchToolConfig | null;
  url_context?: boolean;
  code_execution?: boolean;
  file_search?: FileSearchToolConfig | null;
}

export interface Citation {
  url: string;
  title: string;
  snippet?: string | null;
  domain?: string | null;
  raw_provider_payload?: unknown;
}

export type ContentPart =
  | { kind: "text"; text: string; citations?: Citation[] }
  | { kind: "thinking"; thinking: string; signature?: string }
  | ({ kind: "tool_call" } & ToolCall)
  | ({ kind: "tool_result" } & ToolResult)
  | { kind: "server_tool_use"; id: string; name: string; action?: string | null; input: unknown; raw_provider_payload?: unknown }
  | { kind: "server_tool_result"; tool_use_id: string; name: string; summary: string; is_error: boolean; raw_provider_payload?: unknown }
  | { kind: "image"; source: ImageSource }
  | { kind: "document"; source: unknown; filename?: string };

/** An uploaded image referenced by a message (the bytes stay on the server). */
export interface ImageAttachment {
  type: "attachment";
  attachment_id: string;
  media_type: string;
  filename?: string;
  width?: number;
  height?: number;
}

export type ImageSource = ImageAttachment | { type: "url"; url: string } | { type: "base64"; media_type: string; data: string };

/** One entry of `POST /api/attachments`. */
export interface UploadedAttachment {
  id: string;
  url: string;
  mime_type: string;
  filename: string | null;
  size_bytes: number;
  width: number | null;
  height: number | null;
}

export interface Message {
  role: Role;
  content: ContentPart[];
}

export interface Usage {
  input_tokens: number;
  output_tokens: number;
  total_tokens?: number;
  reasoning_tokens?: number;
  cache_read_tokens?: number;
  cache_creation_tokens?: number;
}

export interface ToolDefinition {
  name: string;
  description: string;
  parameters: unknown;
}

// ----- sessions ------------------------------------------------------------------------------

export type ToolDecision =
  | { decision: "approve" }
  | { decision: "deny"; reason: string }
  | { decision: "need_human_approval"; reason: string };

export type Phase =
  | { phase: "ready" }
  | { phase: "tools_pending"; calls: ToolCall[] }
  | { phase: "awaiting_approval"; calls: ToolCall[]; decisions: Record<string, ToolDecision> }
  | { phase: "idle" }
  | { phase: "halted"; reason: string };

export type SessionStatus = "idle" | "running" | "needs_approval" | "halted";
export type ApprovalPolicy = "auto_all" | "auto_readonly" | "always_ask";

export const APPROVAL_POLICIES: { value: ApprovalPolicy; label: Key; hint: Key }[] = (["auto_all", "auto_readonly", "always_ask"] as const).map((v) => ({
  value: v,
  label: `policy.${v}` as const,
  hint: `policy.${v}.desc` as const,
}));

export interface SessionMeta {
  id: string;
  title: string;
  /** `manual` titles are the user's and are never replaced by generated ones. */
  title_source: "default" | "auto" | "manual";
  provider: string;
  model: string;
  approval_policy: ApprovalPolicy;
  /** Session override of the provider's thinking level. */
  reasoning_effort?: ReasoningEffort;
  /** Session override of provider-hosted server tools. */
  enabled_server_tools?: ServerToolConfig | null;
  lineage: Lineage;
  status: SessionStatus;
  turn: number;
  message_count: number;
  created_at: number;
  updated_at: number;
}

export interface Lineage {
  parent_session_id: string | null;
  parent_tool_call_id: string | null;
  depth: number;
  workspace_root: string | null;
  builtin_groups: string[];
}

export interface SessionSummary extends SessionMeta {
  active: boolean;
  /** The last loop exited with an error. */
  failed?: boolean;
}

/** Why the loop last stopped with an error; cleared when it runs again. */
export interface LoopError {
  message: string;
  at: number;
  /** Set while an automatic retry is scheduled (Unix ms). */
  retry_at: number | null;
  attempt: number;
  max_attempts: number;
}

/** Provider-neutral thinking level (`default`: leave it to the provider / model). */
export type ReasoningEffort = "default" | "off" | "low" | "medium" | "high";

export const REASONING_LEVELS: { value: ReasoningEffort; label: Key; description: Key }[] = (["default", "off", "low", "medium", "high"] as const).map((v) => ({
  value: v,
  label: `reasoning.${v}` as const,
  description: `reasoning.${v}.desc` as const,
}));

export interface ProviderDefaults {
  reasoning_effort: ReasoningEffort;
  temperature: number | null;
  max_output_tokens: number | null;
  context_window: number | null;
  timeout_secs: number;
}

export const DEFAULT_PROVIDER_DEFAULTS: ProviderDefaults = {
  reasoning_effort: "default",
  temperature: null,
  max_output_tokens: null,
  context_window: null,
  timeout_secs: 120,
};

export interface AutoRetry {
  enabled: boolean;
  max_attempts: number;
}

export interface SessionSettings {
  provider: string;
  provider_id?: string;
  model: string;
  max_context_tokens: number;
  max_tokens?: number | null;
  builtin_tools: boolean;
  mcp_servers: { id: string; prefix: string }[];
  widgets?: boolean;
  skills?: { id: string; name: string }[];
}

export interface RawMessage {
  seq: number;
  role: Role;
  content: ContentPart[];
  usage: Usage | null;
  thinking_ms?: number;
}

export interface RequestSummary {
  model: string | null;
  messages: number;
  tools: number;
  bytes: number;
  max_tokens: number | null;
  breakpoints: ({ type: "tools_and_system" } | { type: "after_message"; index: number })[];
  hash: { system: string; tools: string; messages: string; full: string };
}

export interface TurnUsage {
  turn: number;
  seq: number;
  input_tokens: number;
  output_tokens: number;
  cache_read_tokens: number;
  cache_creation_tokens: number;
  prompt_tokens: number;
  cache_hit_rate: number;
  ttft_ms: number | null;
  latency_ms: number | null;
  request: RequestSummary | null;
}

export interface Checkpoint {
  epoch: number;
  up_to_seq: number;
  summary: Message;
}

export interface SessionDetail extends SessionMeta {
  active: boolean;
  last_error?: LoopError | null;
  /** What requests use: the session's level, or the provider's when it is `default`. */
  effective_reasoning_effort?: ReasoningEffort;
  /** Vendor of this session's provider. */
  provider_vendor?: Vendor;
  /** Effective provider-hosted server tools (session override or provider defaults). */
  effective_server_tools?: ServerToolConfig;
  settings: SessionSettings;
  phase: Phase;
  system_prompt: string;
  tools: ToolDefinition[];
  pending_approvals: PendingApproval[];
  pending_steers: number;
  loaded_skills: string[];
  raw: RawMessage[];
  checkpoints: Checkpoint[];
  active_view: { from_seq: number; messages: Message[] };
  usage: {
    ledger: { total_input_tokens: number; total_output_tokens: number; llm_calls: number; last: Usage | null };
    per_turn: TurnUsage[];
    cache_hit_rate: number;
  };
  context: { tokens: number | null; max_tokens: number; compaction_threshold: number };
}

export interface LlmCallStats {
  turn: number;
  seq: number;
  ttft_ms: number | null;
  latency_ms: number;
  usage: Usage | null;
  request: RequestSummary;
}

export type AgentEvent =
  | { event: "turn_started"; turn: number }
  /** `block_id` is `<message seq>:<kind>`, where the block will sit in history. */
  | { event: "text_delta"; step_index: number; block_id: string; delta: string }
  | { event: "thinking_delta"; step_index: number; block_id: string; delta: string }
  | { event: "thinking_end"; block_id: string; duration_ms: number }
  | { event: "server_tool_use"; block_id: string; id: string; name: string; action?: string | null; input: unknown }
  | { event: "server_tool_result"; block_id: string; tool_use_id: string; name: string; summary: string; is_error: boolean }
  | { event: "citation"; block_id: string; citation: Citation }
  | { event: "assistant_message"; message: Message }
  | { event: "tool_started"; call: ToolCall }
  | { event: "tool_finished"; result: ToolResult }
  | { event: "approval_required"; calls: ToolCall[] }
  | { event: "steer_applied"; count: number }
  | { event: "interrupted"; during: string }
  | { event: "compacted"; up_to_index: number; epoch: number }
  | { event: "usage"; usage: Usage }
  | { event: "llm_call"; stats: LlmCallStats }
  | { event: "subagent"; tool_call_id: string; sub_session_id: string; profile: string; inner: AgentEvent }
  | { event: "plan_updated"; steps: PlanStep[] }
  | { event: "title_updated"; session_id: string; title: string }
  | { event: "phase_changed"; phase: string }
  | { event: "completed" }
  | { event: "halted"; reason: string };

/** In-progress (uncommitted) part of the current turn, as the server tracks it. */
export interface LiveTurnWire {
  turn: number | null;
  text: string;
  thinking: string;
  text_block: string | null;
  thinking_block: string | null;
  thinking_ms: number | null;
  thinking_elapsed_ms: number | null;
  running_tools: ToolCall[];
}

export type Envelope =
  | {
      kind: "hello";
      seq: number;
      boot: string;
      resumed: boolean;
      status: SessionStatus;
      active: boolean;
      live: LiveTurnWire;
    }
  | { kind: "agent"; seq: number; event: AgentEvent }
  | { kind: "loop_exited"; seq: number; error: string | null }
  | { kind: "widget"; seq: number; call_id: string; widget: WidgetSpec }
  | { kind: "session_updated"; seq: number; title: string; approval_policy: ApprovalPolicy }
  | { kind: "deleted"; seq: number }
  | { kind: "lagged"; skipped: number };

export interface CreateSessionRequest {
  preset_id?: string;
  reasoning_effort?: ReasoningEffort;
  provider_id?: string;
  model?: string;
  mcp_server_ids?: string[];
  builtin_tools?: boolean;
  system_prompt?: string;
  approval_policy?: ApprovalPolicy;
  max_context_tokens?: number;
  title?: string;
  message?: string;
  attachment_ids?: string[];
  skill_ids?: string[];
  widgets?: boolean;
  builtin_groups?: string[];
  workspace_root?: string;
}

export interface InvokedSkill {
  name: string;
  arguments?: string;
}

export type InputRequest =
  | { type: "steer"; text: string; attachment_ids?: string[]; invoked_skills?: InvokedSkill[] }
  | { type: "interrupt"; text: string; attachment_ids?: string[]; invoked_skills?: InvokedSkill[] }
  | { type: "approval"; call_id: string; decision: "approve" }
  | { type: "approval"; call_id: string; decision: "deny"; reason: string }
  | { type: "approval"; call_id: string; decision: "answer"; content: string }
  | { type: "compact" };

/** Preview attached to a pending call (what the approval card shows). */
export type ApprovalPreview =
  | { kind: "diff"; path: string; created: boolean; diff: string }
  | { kind: "command"; command: string; cwd: string | null; timeout_secs: number }
  | { kind: "skill"; update: boolean; name: string; description: string; body: string; tags: string[]; argument_hint: string | null; execution_mode: string }
  | { kind: "mcp_server"; name: string; transport: string; url: string | null; command: string | null; args: string[]; env: Record<string, string>; headers: Record<string, string> }
  | { kind: "error"; error: string; path?: string };

export interface PendingApproval {
  call: ToolCall;
  reason: string;
  /** `approval` (approve / deny) or `input` (answer). */
  kind: "approval" | "input";
  preview?: ApprovalPreview;
}

// ----- providers -----------------------------------------------------------------------------

export type ProviderKind = "openai" | "anthropic" | "gemini" | "openai_compatible" | "mock";
export type SecretSource = "none" | "inline" | "env";

export const PROVIDER_KINDS: { value: ProviderKind; label: string }[] = [
  { value: "openai", label: "OpenAI (Responses API)" },
  { value: "anthropic", label: "Anthropic" },
  { value: "gemini", label: "Gemini" },
  { value: "openai_compatible", label: "OpenAI-compatible (Chat Completions)" },
  { value: "mock", label: "Mock (offline)" },
];

export interface Provider {
  id: string;
  name: string;
  kind: ProviderKind;
  vendor: Vendor;
  base_url: string | null;
  /** Masked (`AIza…9x2Q`) or an `env:NAME` reference. */
  api_key: string | null;
  api_key_source: SecretSource;
  default_model: string;
  extra_headers: Record<string, string>;
  default_server_tools: ServerToolConfig;
  extra_body: Record<string, unknown> | null;
  is_enabled: boolean;
  auto_retry: AutoRetry;
  defaults: ProviderDefaults;
  created_at: number;
  updated_at: number;
}

export interface ProviderInput {
  name?: string;
  kind?: ProviderKind;
  vendor?: Vendor;
  base_url?: string;
  /** Empty or the masked value keeps the stored key. */
  api_key?: string;
  default_model?: string;
  extra_headers?: Record<string, string>;
  default_server_tools?: ServerToolConfig;
  extra_body?: Record<string, unknown> | null;
  is_enabled?: boolean;
  auto_retry?: AutoRetry;
  defaults?: ProviderDefaults;
  ping?: boolean;
}

export interface ProviderTest {
  ok: boolean;
  latency_ms: number;
  model_checked: string | null;
  model_found: boolean | null;
  models: string[];
  error: string | null;
}

// ----- MCP -----------------------------------------------------------------------------------

export type McpTransport = "streamable_http" | "stdio";

export type McpAuth =
  | { type: "none" }
  | { type: "bearer"; token: string; token_source?: SecretSource }
  | ({ type: "oauth" } & Record<string, unknown>);

export interface McpServer {
  id: string;
  name: string;
  transport: McpTransport;
  url: string | null;
  command: string | null;
  args: string[];
  env: Record<string, string>;
  headers: Record<string, string>;
  auth: McpAuth;
  is_enabled: boolean;
  created_at: number;
  updated_at: number;
  oauth?: OAuthStatus;
}

export interface OAuthStatus {
  status: "connected" | "auth_required";
  issuer: string;
  scopes: string;
  expires_at: number | null;
  client_source: "dcr" | "manual";
  client_id: string;
  has_refresh_token: boolean;
}

export interface McpServerInput {
  name?: string;
  transport?: McpTransport;
  url?: string;
  command?: string;
  args?: string[];
  env?: Record<string, string>;
  headers?: Record<string, string>;
  auth?: { type: "none" } | { type: "bearer"; token?: string } | { type: "oauth"; client_id?: string; client_secret?: string; scopes?: string };
  is_enabled?: boolean;
}

export interface McpToolInfo {
  name: string;
  title: string | null;
  description: string | null;
  read_only: boolean;
  input_schema: unknown;
}

export interface McpTest {
  ok: boolean;
  latency_ms: number;
  server_info: { name: string; version: string; protocol_version: string; instructions: string | null } | null;
  tools: McpToolInfo[];
  error: string | null;
}

export interface ToolCallResult {
  ok: boolean;
  latency_ms: number;
  is_error: boolean;
  content: unknown;
  structured_content: unknown;
  text: string | null;
  error: string | null;
}

// ----- presets -------------------------------------------------------------------------------

export interface Preset {
  id: string;
  name: string;
  description: string;
  provider_id: string;
  model: string;
  system_prompt: string;
  mcp_server_ids: string[];
  builtin_tools: boolean;
  approval_policy: ApprovalPolicy;
  skill_ids: string[];
  builtin_groups: string[];
  workspace_root: string | null;
  reasoning_effort: ReasoningEffort;
  created_at: number;
  updated_at: number;
}

export type PresetInput = Partial<Omit<Preset, "id" | "created_at" | "updated_at">>;

// ----- skills --------------------------------------------------------------------------------

export interface Skill {
  id: string;
  name: string;
  description: string;
  body: string;
  allowed_tools: string[];
  tags: string[];
  source: { kind: "file"; path: string } | { kind: "db" };
  updated_at: number;
  editable: boolean;
  user_invocable: boolean;
  argument_hint: string | null;
  execution_mode: "inline" | "subagent";
  subagent_profile: string | null;
}

export interface SkillInput {
  name?: string;
  description?: string;
  body?: string;
  allowed_tools?: string[];
  tags?: string[];
  user_invocable?: boolean;
  argument_hint?: string;
  execution_mode?: "inline" | "subagent";
  subagent_profile?: string;
}

export interface SkillsReload {
  file_skills: number;
  warnings: string[];
  dirs: string[];
}

// ----- widgets -------------------------------------------------------------------------------

export type WidgetType = "metrics_chart" | "data_table" | "interactive_form" | "approval_diff" | "action_chips";

/** Arguments of a `render_widget` call. */
export interface WidgetSpec {
  widget_type: WidgetType | string;
  title?: string;
  data: Record<string, unknown>;
}

// ----- plan, commands, memories ------------------------------------------------------------

export type PlanStatus = "pending" | "in_progress" | "done" | "skipped";

export interface PlanStep {
  id: string;
  title: string;
  status: PlanStatus;
}

export interface CommandInfo {
  name: string;
  kind: "command" | "skill";
  description: string;
  argument_hint: string | null;
  subagent: boolean;
  needs_session: boolean;
}

export interface CommandResult {
  message: string;
  effect: { navigate_session?: string; download?: string };
}

export interface Memory {
  id: string;
  title: string;
  content: string;
  tags: string[];
  scope: string;
  created_at: number;
  updated_at: number;
  score?: number;
}

/** Builtin tool groups; labels and hints are `groups.<id>` / `groups.<id>.hint` in the dictionaries. */
export const BUILTIN_GROUPS: { id: "self_config" | "core_ux" | "memory" | "utilities" | "subagent" | "workspace" }[] = [
  { id: "self_config" },
  { id: "core_ux" },
  { id: "memory" },
  { id: "utilities" },
  { id: "subagent" },
  { id: "workspace" },
];
