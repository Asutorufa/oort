import { request } from "./client";
import type {
  ReasoningEffort,
  CreateSessionRequest,
  InputRequest,
  McpServer,
  McpServerInput,
  McpTest,
  Preset,
  PresetInput,
  Provider,
  ProviderInput,
  ProviderTest,
  SessionDetail,
  SessionSummary,
  ApprovalPolicy,
  Skill,
  SkillInput,
  SkillsReload,
  OAuthStatus,
  ToolCallResult,
  CommandInfo,
  CommandResult,
  Memory,
} from "./types";

export { ApiError, errorText } from "./client";

export const sessions = {
  list: () => request<SessionSummary[]>("GET", "/sessions"),
  get: (id: string) => request<SessionDetail>("GET", `/sessions/${id}`),
  create: (req: CreateSessionRequest) => request<SessionDetail>("POST", "/sessions", req),
  patch: (id: string, req: { title?: string; approval_policy?: ApprovalPolicy; reasoning_effort?: ReasoningEffort; model?: string }) =>
    request<SessionDetail>("PATCH", `/sessions/${id}`, req),
  remove: (id: string) => request<void>("DELETE", `/sessions/${id}`),
  exportUrl: (id: string) => `/api/sessions/${id}/export`,
  input: (id: string, input: InputRequest) => request<void>("POST", `/sessions/${id}/input`, input),
  cancel: (id: string) => request<void>("POST", `/sessions/${id}/cancel`),
  retry: (id: string) => request<void>("POST", `/sessions/${id}/retry`),
  eventsUrl: (id: string) => `/api/sessions/${id}/events`,
  children: (id: string) => request<SessionSummary[]>("GET", `/sessions/${id}/children`),
};

export const commands = {
  list: () => request<CommandInfo[]>("GET", "/commands"),
  run: (name: string, sessionId?: string | null, args?: string) =>
    request<CommandResult>("POST", "/commands/run", { session_id: sessionId ?? undefined, name, arguments: args || undefined }),
};

export const memories = {
  search: (q = "", tag = "") => request<Memory[]>("GET", `/memories?q=${encodeURIComponent(q)}&tag=${encodeURIComponent(tag)}`),
  create: (m: Partial<Memory>) => request<Memory>("POST", "/memories", m),
  update: (id: string, m: Partial<Memory>) => request<Memory>("PUT", `/memories/${id}`, m),
  remove: (id: string) => request<void>("DELETE", `/memories/${id}`),
};

export const providers = {
  list: () => request<Provider[]>("GET", "/providers"),
  create: (input: ProviderInput) => request<Provider>("POST", "/providers", input),
  update: (id: string, input: ProviderInput) => request<Provider>("PUT", `/providers/${id}`, input),
  remove: (id: string) => request<void>("DELETE", `/providers/${id}`),
  /** Tests the saved provider with optional unsaved overrides, or an unsaved form (`id` null). */
  test: (id: string | null, input: ProviderInput = {}) =>
    request<ProviderTest>("POST", id ? `/providers/${id}/test` : "/providers/test", input),
  /** Model ids the provider offers (`id` null: an unsaved form). */
  models: (id: string | null, input?: ProviderInput) =>
    request<{ models: string[] }>("POST", id ? `/providers/${id}/models` : "/providers/models", input ?? {}),
};

export const mcpServers = {
  list: () => request<McpServer[]>("GET", "/mcp-servers"),
  create: (input: McpServerInput) => request<McpServer>("POST", "/mcp-servers", input),
  update: (id: string, input: McpServerInput) => request<McpServer>("PUT", `/mcp-servers/${id}`, input),
  remove: (id: string) => request<void>("DELETE", `/mcp-servers/${id}`),
  test: (id: string | null, input: McpServerInput = {}) =>
    request<McpTest>("POST", id ? `/mcp-servers/${id}/test` : "/mcp-servers/test", input),
  callTool: (id: string, tool_name: string, args: unknown, timeout_ms?: number) =>
    request<ToolCallResult>("POST", `/mcp-servers/${id}/tools/call`, {
      tool_name,
      arguments: args,
      timeout_ms,
    }),
};

export const oauth = {
  status: (id: string) => request<OAuthStatus | null>("GET", `/mcp-servers/${id}/oauth`),
  start: (id: string) => request<{ authorization_url: string; state: string }>("POST", `/mcp-servers/${id}/oauth/start`),
  disconnect: (id: string) => request<void>("POST", `/mcp-servers/${id}/oauth/disconnect`),
};

export const skills = {
  list: () => request<Skill[]>("GET", "/skills"),
  create: (input: SkillInput) => request<Skill>("POST", "/skills", input),
  update: (id: string, input: SkillInput) => request<Skill>("PUT", `/skills/${encodeURIComponent(id)}`, input),
  remove: (id: string) => request<void>("DELETE", `/skills/${encodeURIComponent(id)}`),
  reload: () => request<SkillsReload>("POST", "/skills/reload"),
};

export const presets = {
  list: () => request<Preset[]>("GET", "/presets"),
  create: (input: PresetInput) => request<Preset>("POST", "/presets", input),
  update: (id: string, input: PresetInput) => request<Preset>("PUT", `/presets/${id}`, input),
  remove: (id: string) => request<void>("DELETE", `/presets/${id}`),
};
