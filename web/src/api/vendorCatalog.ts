import type { ProviderKind, Vendor } from "./types";

export type ServerToolCapability = "web_search" | "url_context" | "code_execution" | "file_search";

export interface ProtocolOption {
  kind: ProviderKind;
  label: string;
  recommended?: boolean;
  defaultBaseUrl: string;
  serverTools: Partial<Record<ServerToolCapability, string>>;
}

export interface VendorPreset {
  id: Vendor;
  name: string;
  sub: string;
  defaultModel: string;
  env: string;
  apiKeyOptional?: boolean;
  baseRequired?: boolean;
  protocols: ProtocolOption[];
}

export const VENDOR_PRESETS: VendorPreset[] = [
  {
    id: "openai",
    name: "OpenAI",
    sub: "GPT / o-series",
    defaultModel: "gpt-5.1",
    env: "OPENAI_API_KEY",
    protocols: [
      {
        kind: "openai",
        label: "Responses API",
        recommended: true,
        defaultBaseUrl: "https://api.openai.com/v1",
        serverTools: {
          web_search: "web_search",
          code_execution: "code_interpreter",
          file_search: "file_search",
        },
      },
      {
        kind: "openai_compatible",
        label: "Chat Completions",
        defaultBaseUrl: "https://api.openai.com/v1",
        serverTools: {
          web_search: "web_search_options",
        },
      },
    ],
  },
  {
    id: "anthropic",
    name: "Anthropic",
    sub: "Claude",
    defaultModel: "claude-sonnet-4-5",
    env: "ANTHROPIC_API_KEY",
    protocols: [
      {
        kind: "anthropic",
        label: "Messages API",
        recommended: true,
        defaultBaseUrl: "https://api.anthropic.com",
        serverTools: {
          web_search: "web_search_20250305",
          url_context: "web_fetch_20250910",
          code_execution: "code_execution_20250825",
        },
      },
    ],
  },
  {
    id: "gemini",
    name: "Gemini",
    sub: "Google AI Studio",
    defaultModel: "gemini-2.5-flash",
    env: "GEMINI_API_KEY",
    protocols: [
      {
        kind: "gemini",
        label: "Gemini API",
        recommended: true,
        defaultBaseUrl: "https://generativelanguage.googleapis.com/v1beta",
        serverTools: {
          web_search: "google_search",
          url_context: "url_context",
          code_execution: "code_execution",
        },
      },
    ],
  },
  {
    id: "deepseek",
    name: "DeepSeek",
    sub: "deepseek-flash",
    defaultModel: "deepseek-flash",
    env: "DEEPSEEK_API_KEY",
    protocols: [
      {
        kind: "openai",
        label: "Responses API",
        recommended: true,
        defaultBaseUrl: "https://api.deepseek.com",
        serverTools: {
          web_search: "deepseek_web_search",
          url_context: "deepseek_read_url",
        },
      },
      {
        kind: "anthropic",
        label: "Anthropic API",
        defaultBaseUrl: "https://api.deepseek.com/anthropic",
        serverTools: {
          web_search: "deepseek_web_search",
          url_context: "deepseek_read_url",
        },
      },
      {
        kind: "openai_compatible",
        label: "Chat Completions",
        defaultBaseUrl: "https://api.deepseek.com",
        serverTools: {
          web_search: "deepseek_web_search",
          url_context: "deepseek_read_url",
        },
      },
    ],
  },
  {
    id: "openrouter",
    name: "OpenRouter",
    sub: "Multi-model router",
    defaultModel: "openai/gpt-4o",
    env: "OPENROUTER_API_KEY",
    protocols: [
      {
        kind: "openai_compatible",
        label: "Chat Completions",
        recommended: true,
        defaultBaseUrl: "https://openrouter.ai/api/v1",
        serverTools: {
          web_search: "openrouter:web_search",
          url_context: "openrouter:web_fetch",
        },
      },
      {
        kind: "openai",
        label: "Responses API",
        defaultBaseUrl: "https://openrouter.ai/api/v1",
        serverTools: {
          web_search: "openrouter:web_search",
          url_context: "openrouter:web_fetch",
        },
      },
    ],
  },
  {
    id: "qwen",
    name: "Qwen",
    sub: "DashScope",
    defaultModel: "qwen-max",
    env: "DASHSCOPE_API_KEY",
    protocols: [
      {
        kind: "openai_compatible",
        label: "Chat Completions",
        recommended: true,
        defaultBaseUrl: "https://dashscope.aliyuncs.com/compatible-mode/v1",
        serverTools: {
          web_search: "enable_search",
        },
      },
      {
        kind: "openai",
        label: "Responses API",
        defaultBaseUrl: "https://dashscope.aliyuncs.com/api/v2/apps/protocols/compatible-mode/v1",
        serverTools: {
          web_search: "web_search",
          url_context: "web_extractor",
          code_execution: "code_interpreter",
        },
      },
    ],
  },
  {
    id: "ollama",
    name: "Ollama",
    sub: "Local models",
    defaultModel: "llama3.1:8b",
    env: "OLLAMA_API_KEY",
    apiKeyOptional: true,
    protocols: [
      {
        kind: "openai_compatible",
        label: "Chat Completions",
        recommended: true,
        defaultBaseUrl: "http://localhost:11434/v1",
        serverTools: {},
      },
    ],
  },
  {
    id: "custom",
    name: "Custom",
    sub: "Any endpoint",
    defaultModel: "",
    env: "CUSTOM_API_KEY",
    baseRequired: true,
    protocols: [
      {
        kind: "openai_compatible",
        label: "Chat Completions",
        recommended: true,
        defaultBaseUrl: "",
        serverTools: {},
      },
      {
        kind: "openai",
        label: "Responses API",
        defaultBaseUrl: "",
        serverTools: {},
      },
      {
        kind: "anthropic",
        label: "Anthropic",
        defaultBaseUrl: "",
        serverTools: {},
      },
      {
        kind: "gemini",
        label: "Gemini",
        defaultBaseUrl: "",
        serverTools: {},
      },
    ],
  },
];

export function getVendorPreset(vendor: Vendor): VendorPreset {
  return VENDOR_PRESETS.find((v) => v.id === vendor) ?? VENDOR_PRESETS[VENDOR_PRESETS.length - 1];
}

export function getProtocolOption(vendor: Vendor, kind: ProviderKind | string): ProtocolOption | null {
  const preset = getVendorPreset(vendor);
  return preset.protocols.find((p) => p.kind === kind) ?? null;
}

export function supportsWebSearch(vendor: Vendor | undefined, kind: ProviderKind | string): boolean {
  if (!vendor) return false;
  const proto = getProtocolOption(vendor, kind);
  return !!proto?.serverTools.web_search;
}
