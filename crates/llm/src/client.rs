use std::future::Future;
use std::pin::Pin;

use futures_util::{Stream, StreamExt};

use crate::Error;
use crate::anthropic::AnthropicClient;
use crate::gemini::GeminiClient;
use crate::openai::OpenAIClient;
use crate::openai_responses::OpenAIResponsesClient;
use crate::types::{ChatRequest, ChatResponse, StreamCollector, StreamEvent};

/// Boxed, `Send` stream of incremental [`StreamEvent`]s returned by [`LlmClient::chat_stream`].
pub type ChatStream = Pin<Box<dyn Stream<Item = Result<StreamEvent, Error>> + Send + 'static>>;

/// Common async interface implemented by all LLM API clients in this crate.
pub trait LlmClient: Send + Sync {
    /// Sends a non-streaming chat request and returns the full [`ChatResponse`].
    fn chat(
        &self,
        request: ChatRequest,
    ) -> impl Future<Output = Result<ChatResponse, Error>> + Send;

    /// Sends a streaming chat request and returns a [`ChatStream`] of [`StreamEvent`]s.
    fn chat_stream(
        &self,
        request: ChatRequest,
    ) -> impl Future<Output = Result<ChatStream, Error>> + Send;

    /// Lists available model identifiers from the provider API.
    fn list_models(&self) -> impl Future<Output = Result<Vec<String>, Error>> + Send;
}

/// Dynamic dispatch enum wrapping any concrete LLM client.
#[derive(Debug, Clone)]
pub enum Provider {
    OpenAI(OpenAIClient),
    OpenAIResponses(OpenAIResponsesClient),
    Anthropic(AnthropicClient),
    Gemini(GeminiClient),
}

impl Provider {
    pub fn model(&self) -> &str {
        match self {
            Self::OpenAI(c) => c.model(),
            Self::OpenAIResponses(c) => c.model(),
            Self::Anthropic(c) => c.model(),
            Self::Gemini(c) => c.model(),
        }
    }

    pub fn set_model(&mut self, model: impl Into<String>) {
        match self {
            Self::OpenAI(c) => c.set_model(model),
            Self::OpenAIResponses(c) => c.set_model(model),
            Self::Anthropic(c) => c.set_model(model),
            Self::Gemini(c) => c.set_model(model),
        }
    }
}

impl From<OpenAIClient> for Provider {
    fn from(client: OpenAIClient) -> Self {
        Self::OpenAI(client)
    }
}

impl From<OpenAIResponsesClient> for Provider {
    fn from(client: OpenAIResponsesClient) -> Self {
        Self::OpenAIResponses(client)
    }
}

impl From<AnthropicClient> for Provider {
    fn from(client: AnthropicClient) -> Self {
        Self::Anthropic(client)
    }
}

impl From<GeminiClient> for Provider {
    fn from(client: GeminiClient) -> Self {
        Self::Gemini(client)
    }
}

impl LlmClient for Provider {
    async fn chat(&self, request: ChatRequest) -> Result<ChatResponse, Error> {
        match self {
            Self::OpenAI(c) => c.chat(request).await,
            Self::OpenAIResponses(c) => c.chat(request).await,
            Self::Anthropic(c) => c.chat(request).await,
            Self::Gemini(c) => c.chat(request).await,
        }
    }

    async fn chat_stream(&self, request: ChatRequest) -> Result<ChatStream, Error> {
        match self {
            Self::OpenAI(c) => c.chat_stream(request).await,
            Self::OpenAIResponses(c) => c.chat_stream(request).await,
            Self::Anthropic(c) => c.chat_stream(request).await,
            Self::Gemini(c) => c.chat_stream(request).await,
        }
    }

    async fn list_models(&self) -> Result<Vec<String>, Error> {
        match self {
            Self::OpenAI(c) => c.list_models().await,
            Self::OpenAIResponses(c) => c.list_models().await,
            Self::Anthropic(c) => c.list_models().await,
            Self::Gemini(c) => c.list_models().await,
        }
    }
}

/// Drains a [`Stream`] of [`StreamEvent`]s and assembles a complete [`ChatResponse`].
pub async fn collect_stream<S>(stream: S) -> Result<ChatResponse, Error>
where
    S: Stream<Item = Result<StreamEvent, Error>>,
{
    futures_util::pin_mut!(stream);
    let mut collector = StreamCollector::new();
    while let Some(item) = stream.next().await {
        let event = item?;
        collector.push(&event);
    }
    collector.finish()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Message;
    use mockito::Server;

    #[tokio::test]
    async fn test_provider_enum_dispatches_chat_and_set_model() {
        let mut server = Server::new_async().await;
        let mock = server
            .mock("POST", "/chat/completions")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                r#"{"id":"c1","model":"gpt-4.1","choices":[{"message":{"role":"assistant","content":"hi"},"finish_reason":"stop"}]}"#,
            )
            .create_async()
            .await;

        let mut provider: Provider = OpenAIClient::new("sk-test", "gpt-4o")
            .with_base_url(server.url())
            .into();
        provider.set_model("gpt-4.1");
        assert_eq!(provider.model(), "gpt-4.1");

        let resp = provider
            .chat(ChatRequest::new(vec![Message::user("hello")]))
            .await
            .unwrap();
        mock.assert_async().await;
        assert_eq!(resp.content, "hi");
    }
}
