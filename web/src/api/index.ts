import { request } from "./client";
import type {
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
  ToolCallResult,
} from "./types";

export { ApiError, errorText } from "./client";

export const sessions = {
  list: () => request<SessionSummary[]>("GET", "/sessions"),
  get: (id: string) => request<SessionDetail>("GET", `/sessions/${id}`),
  create: (req: CreateSessionRequest) => request<SessionDetail>("POST", "/sessions", req),
  patch: (id: string, req: { title?: string; approval_policy?: ApprovalPolicy }) =>
    request<SessionDetail>("PATCH", `/sessions/${id}`, req),
  remove: (id: string) => request<void>("DELETE", `/sessions/${id}`),
  exportUrl: (id: string) => `/api/sessions/${id}/export`,
  input: (id: string, input: InputRequest) => request<void>("POST", `/sessions/${id}/input`, input),
  cancel: (id: string) => request<void>("POST", `/sessions/${id}/cancel`),
  eventsUrl: (id: string) => `/api/sessions/${id}/events`,
};

export const providers = {
  list: () => request<Provider[]>("GET", "/providers"),
  create: (input: ProviderInput) => request<Provider>("POST", "/providers", input),
  update: (id: string, input: ProviderInput) => request<Provider>("PUT", `/providers/${id}`, input),
  remove: (id: string) => request<void>("DELETE", `/providers/${id}`),
  /** Tests the saved provider with optional unsaved overrides, or an unsaved form (`id` null). */
  test: (id: string | null, input: ProviderInput = {}) =>
    request<ProviderTest>("POST", id ? `/providers/${id}/test` : "/providers/test", input),
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

export const presets = {
  list: () => request<Preset[]>("GET", "/presets"),
  create: (input: PresetInput) => request<Preset>("POST", "/presets", input),
  update: (id: string, input: PresetInput) => request<Preset>("PUT", `/presets/${id}`, input),
  remove: (id: string) => request<void>("DELETE", `/presets/${id}`),
};
