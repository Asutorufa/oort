// Mirrors crates/server/src/api.rs and the serde shapes of oort-agent / oort-llm types.

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
export type ProviderKind = "anthropic" | "openai" | "openai_responses" | "gemini" | "mock";
export type ApprovalMode = "mutating" | "never";

export interface SessionMeta {
  id: string;
  title: string;
  provider: string;
  model: string;
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
  provider: ProviderKind;
  model: string;
  max_context_tokens: number;
  max_tokens?: number;
  approval: ApprovalMode;
}

export interface ToolDefinition {
  name: string;
  description: string;
  parameters: unknown;
}

export interface RawMessage {
  seq: number;
  role: Role;
  content: ContentPart[];
  usage: Usage | null;
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
  checkpoints: { epoch: number; up_to_seq: number; summary: Message }[];
  active_view: { from_seq: number; messages: Message[] };
  usage: {
    ledger: { total_input_tokens: number; total_output_tokens: number; llm_calls: number; last: Usage | null };
    per_turn: TurnUsage[];
    cache_hit_rate: number;
  };
  context: { tokens: number | null; max_tokens: number; compaction_threshold: number };
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
  | { event: "phase_changed"; phase: string }
  | { event: "completed" }
  | { event: "halted"; reason: string };

export type Envelope =
  | { kind: "agent"; seq: number; event: AgentEvent }
  | { kind: "loop_exited"; seq: number; error: string | null }
  | { kind: "lagged"; skipped: number };

export interface CreateSessionRequest {
  title?: string;
  provider: ProviderKind;
  model: string;
  system_prompt: string;
  max_context_tokens?: number;
  approval: ApprovalMode;
  message?: string;
}

export type InputRequest =
  | { type: "steer"; text: string }
  | { type: "interrupt"; text: string }
  | { type: "approval"; call_id: string; decision: "approve" }
  | { type: "approval"; call_id: string; decision: "deny"; reason: string }
  | { type: "compact" };
