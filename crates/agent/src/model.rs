//! Object-safe LLM boundary used by the loop.
//!
//! `oort_llm::LlmClient` uses `impl Future` returns and therefore cannot be used as
//! `dyn LlmClient`. [`ModelClient`] is the `dyn`-compatible equivalent, with a blanket impl
//! for every `LlmClient`, so any provider (or `oort_llm::Provider`) can be passed as
//! `Arc<dyn ModelClient>` directly.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use oort_llm::{ChatRequest, ChatStream, LlmClient};

/// Streaming chat model, as seen by the agent loop.
#[async_trait]
pub trait ModelClient: Send + Sync {
    /// Opens a streaming chat request.
    async fn stream(&self, request: ChatRequest) -> Result<ChatStream, oort_llm::Error>;
}

#[async_trait]
impl<T: LlmClient> ModelClient for T {
    async fn stream(&self, request: ChatRequest) -> Result<ChatStream, oort_llm::Error> {
        self.chat_stream(request).await
    }
}

/// Decorator that retries opening a stream on retryable errors (429, 5xx, timeouts)
/// with exponential backoff. Mid-stream failures are not retried.
pub struct RetryingModel {
    inner: Arc<dyn ModelClient>,
    max_retries: u32,
    base_delay: Duration,
}

impl RetryingModel {
    pub fn new(inner: Arc<dyn ModelClient>, max_retries: u32, base_delay: Duration) -> Self {
        Self {
            inner,
            max_retries,
            base_delay,
        }
    }
}

#[async_trait]
impl ModelClient for RetryingModel {
    async fn stream(&self, request: ChatRequest) -> Result<ChatStream, oort_llm::Error> {
        let mut attempt = 0;
        loop {
            match self.inner.stream(request.clone()).await {
                Err(err) if err.is_retryable() && attempt < self.max_retries => {
                    let delay = self.base_delay * 2u32.saturating_pow(attempt);
                    tracing::warn!(attempt, ?delay, error = %err, "retrying LLM request");
                    tokio::time::sleep(delay).await;
                    attempt += 1;
                }
                other => return other,
            }
        }
    }
}
