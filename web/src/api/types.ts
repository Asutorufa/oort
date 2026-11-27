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

export type ContentPart =
  | { kind: "text"; text: string }
  | { kind: "thinking"; thinking: string; signature?: string }
  | ({ kind: "tool_call" } & ToolCall)
  | ({ kind: "tool_result" } & ToolResult)
  | { kind: "image"; source: unknown }
  | { kind: "document"; source: unknown; filename?: string };

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

export const APPROVAL_POLICIES: { value: ApprovalPolicy; label: string; hint: string }[] = [
  { value: "auto_all", label: "Auto All", hint: "every tool runs without asking" },
  { value: "auto_readonly", label: "Auto Read-Only", hint: "read-only tools run; others ask" },
  { value: "always_ask", label: "Always Ask", hint: "every tool call asks" },
];

export interface SessionMeta {
  id: string;
  title: string;
  provider: string;
  model: string;
  approval_policy: ApprovalPolicy;
  status: SessionStatus;
  turn: number;
  message_count: number;
  created_at: number;
  updated_at: number;
}

export interface SessionSummary extends SessionMeta {
  active: boolean;
}

export interface SessionSettings {
  provider: string;
  provider_id?: string;
  model: string;
  max_context_tokens: number;
  max_tokens?: number | null;
  builtin_tools: boolean;
  mcp_servers: { id: string; prefix: string }[];
}

export interface RawMessage {
  seq: number;
  role: Role;
  content: ContentPart[];
  usage: Usage | null;
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
  settings: SessionSettings;
  phase: Phase;
  system_prompt: string;
  tools: ToolDefinition[];
  pending_approvals: { call: ToolCall; reason: string }[];
  pending_steers: number;
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
  | { event: "text_delta"; delta: string }
  | { event: "thinking_delta"; delta: string }
  | { event: "assistant_message"; message: Message }
  | { event: "tool_started"; call: ToolCall }
  | { event: "tool_finished"; result: ToolResult }
  | { event: "approval_required"; calls: ToolCall[] }
  | { event: "steer_applied"; count: number }
  | { event: "interrupted"; during: string }
  | { event: "compacted"; up_to_index: number; epoch: number }
  | { event: "usage"; usage: Usage }
  | { event: "llm_call"; stats: LlmCallStats }
  | { event: "phase_changed"; phase: string }
  | { event: "completed" }
  | { event: "halted"; reason: string };

/** In-progress (uncommitted) part of the current turn, as the server tracks it. */
export interface LiveTurnWire {
  turn: number | null;
  text: string;
  thinking: string;
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
  | { kind: "session_updated"; seq: number; title: string; approval_policy: ApprovalPolicy }
  | { kind: "deleted"; seq: number }
  | { kind: "lagged"; skipped: number };

export interface CreateSessionRequest {
  preset_id?: string;
  provider_id?: string;
  model?: string;
  mcp_server_ids?: string[];
  builtin_tools?: boolean;
  system_prompt?: string;
  approval_policy?: ApprovalPolicy;
  max_context_tokens?: number;
  title?: string;
  message?: string;
}

export type InputRequest =
  | { type: "steer"; text: string }
  | { type: "interrupt"; text: string }
  | { type: "approval"; call_id: string; decision: "approve" }
  | { type: "approval"; call_id: string; decision: "deny"; reason: string }
  | { type: "compact" };

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
  base_url: string | null;
  /** Masked (`AIza…9x2Q`) or an `env:NAME` reference. */
  api_key: string | null;
  api_key_source: SecretSource;
  default_model: string;
  extra_headers: Record<string, string>;
  is_enabled: boolean;
  created_at: number;
  updated_at: number;
}

export interface ProviderInput {
  name?: string;
  kind?: ProviderKind;
  base_url?: string;
  /** Empty or the masked value keeps the stored key. */
  api_key?: string;
  default_model?: string;
  extra_headers?: Record<string, string>;
  is_enabled?: boolean;
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
}

export interface McpServerInput {
  name?: string;
  transport?: McpTransport;
  url?: string;
  command?: string;
  args?: string[];
  env?: Record<string, string>;
  headers?: Record<string, string>;
  auth?: { type: "none" } | { type: "bearer"; token?: string };
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
  created_at: number;
  updated_at: number;
}

export type PresetInput = Partial<Omit<Preset, "id" | "created_at" | "updated_at">>;
