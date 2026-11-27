//! Pure-HTTP Rust LLM API clients for OpenAI, Gemini, and Anthropic.
//!
//! Designed as a standalone foundation crate for LLM agents, supporting:
//! - Non-streaming and SSE streaming responses across all providers
//! - Tool / function calling (including streamed JSON argument assembly)
//! - Extended thinking / reasoning tokens and signatures
//! - Multimodal image inputs
//! - Deterministic serialization and provider-specific prompt prefix caching

pub mod anthropic;
pub mod client;
pub mod error;
pub mod gemini;
mod http;
pub mod openai;
pub mod openai_responses;
pub mod sse;
pub mod types;

pub use anthropic::AnthropicClient;
pub use client::{ChatStream, LlmClient, Provider, collect_stream};
pub use error::Error;
pub use gemini::GeminiClient;
pub use openai::{CompletionRequest, OpenAIClient};
pub use openai_responses::OpenAIResponsesClient;
pub use types::{
    CacheBreakpoint, CacheTtl, ChatRequest, ChatResponse, ContentPart, DocumentSource, ImageSource, Message,
    PromptCacheConfig, ReasoningEffort, Role, StopReason, StreamCollector, StreamEvent,
    ThinkingConfig, ToolCall, ToolChoice, ToolDefinition, ToolResult, Usage,
};

#[cfg(test)]
mod prefix_cache_tests;
