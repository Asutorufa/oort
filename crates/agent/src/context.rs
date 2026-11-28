//! Separation of the raw event history (`AgentState`) from the request view sent to the LLM.

use oort_llm::{
    CacheBreakpoint, ChatRequest, Message, PromptCacheConfig, Role, ThinkingConfig, ToolChoice,
};

use crate::compact::{CompactionPolicy, select_boundary};
use crate::state::AgentState;
use crate::tool::FrozenToolset;

/// Builds LLM requests from state and decides when compaction is due.
///
/// # Invariant
/// For a fixed epoch (same latest checkpoint), `build_request(state_n+1)` must serialize to a
/// strict extension of `build_request(state_n)`: identical system, tools, and parameters, with
/// `messages` only growing at the tail.
pub trait ContextManager: Send + Sync {
    fn build_request(&self, state: &AgentState, tools: &FrozenToolset) -> ChatRequest;

    /// Returns the `up_to_index` for a new checkpoint if compaction should run now.
    /// `forced` is set for manual (`LoopInput::Compact`) requests.
    fn plan_compaction(&self, state: &AgentState, forced: bool) -> Option<usize>;
}

/// Request parameters that stay fixed for the whole session.
#[derive(Debug, Clone, Default)]
pub struct RequestParams {
    pub model: Option<String>,
    pub max_tokens: Option<u32>,
    pub temperature: Option<f32>,
    pub thinking: Option<ThinkingConfig>,
    pub tool_choice: Option<ToolChoice>,
    pub prompt_cache: Option<PromptCacheConfig>,
    /// When set, replaces `thinking` with its current value on every request: the thinking level
    /// is a request parameter outside the cached prefix, so it may change between turns.
    pub live_thinking: Option<LiveThinking>,
}

/// Shared, changeable thinking setting (see [`RequestParams::live_thinking`]).
pub type LiveThinking = std::sync::Arc<std::sync::RwLock<Option<ThinkingConfig>>>;

/// Default view: `[System, Tools, latest_summary?, messages[up_to_index..]]` with cache
/// breakpoints at the static prefix, the previous request's tail, and the current tail.
#[derive(Debug, Clone, Default)]
pub struct DefaultContextManager {
    pub params: RequestParams,
    pub compaction: Option<CompactionPolicy>,
}

impl DefaultContextManager {
    pub fn new(params: RequestParams) -> Self {
        Self {
            params,
            compaction: None,
        }
    }

    pub fn with_compaction(mut self, policy: CompactionPolicy) -> Self {
        self.compaction = Some(policy);
        self
    }
}

impl ContextManager for DefaultContextManager {
    fn build_request(&self, state: &AgentState, tools: &FrozenToolset) -> ChatRequest {
        let messages = state.active_view();

        let p = &self.params;
        let system = state.system_prompt();
        let mut req = ChatRequest::new(messages).with_tools(tools.definitions().to_vec());
        req.system = (!system.is_empty()).then(|| system.to_string());
        req.model = p.model.clone();
        req.max_tokens = p.max_tokens;
        req.temperature = p.temperature;
        req.thinking = match &p.live_thinking {
            Some(live) => live.read().unwrap_or_else(|e| e.into_inner()).clone(),
            None => p.thinking.clone(),
        };
        req.tool_choice = p.tool_choice.clone();
        req.prompt_cache = p.prompt_cache.clone();
        if p.prompt_cache.as_ref().is_some_and(|c| c.enabled) {
            req.cache_breakpoints = cache_breakpoints(&req.messages);
        }
        req
    }

    fn plan_compaction(&self, state: &AgentState, forced: bool) -> Option<usize> {
        let keep = self
            .compaction
            .as_ref()
            .map_or(0, |p| p.keep_recent_messages);
        let due = forced
            || self
                .compaction
                .as_ref()
                .is_some_and(|p| p.is_over_threshold(state));
        if !due {
            return None;
        }
        let from = state.latest_checkpoint().map_or(0, |c| c.up_to_index);
        select_boundary(state.messages(), from, keep)
    }
}

/// Static prefix + the last message before the newest assistant reply (the previous request's
/// tail, already cached) + the current tail (written for the next request).
fn cache_breakpoints(messages: &[Message]) -> Vec<CacheBreakpoint> {
    let mut bps = vec![CacheBreakpoint::ToolsAndSystem];
    if let Some(last_assistant) = messages.iter().rposition(|m| m.role == Role::Assistant)
        && last_assistant > 0
    {
        bps.push(CacheBreakpoint::AfterMessage {
            index: last_assistant - 1,
        });
    }
    if !messages.is_empty() {
        bps.push(CacheBreakpoint::AfterMessage {
            index: messages.len() - 1,
        });
    }
    bps.dedup();
    bps
}
