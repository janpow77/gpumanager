use async_trait::async_trait;

use super::types::{ChatCompletionChunk, ChatCompletionRequest, ChatCompletionResponse};

/// Fehlertyp fuer Provider-Operationen
#[derive(Debug, thiserror::Error)]
pub enum ProviderError {
    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),
    #[error("API error: {status} {message}")]
    Api { status: u16, message: String },
    #[error("Streaming error: {0}")]
    Stream(String),
    #[error("Configuration error: {0}")]
    Config(String),
}

/// Trait fuer LLM-Provider
#[async_trait]
pub trait LlmProvider: Send + Sync {
    /// Provider-Name
    fn name(&self) -> &str;

    /// Prueft ob dieser Provider das angegebene Modell unterstuetzt
    fn supports_model(&self, model: &str) -> bool;

    /// Chat-Completion-Request senden (nicht-streamend)
    async fn chat_completion(
        &self,
        request: &ChatCompletionRequest,
    ) -> Result<ChatCompletionResponse, ProviderError>;

    /// Streamenden Chat-Completion-Request senden
    /// Gibt Chunks als Vec zurueck (vereinfacht; echtes Streaming via SSE in api.rs)
    async fn chat_completion_stream(
        &self,
        request: &ChatCompletionRequest,
    ) -> Result<Vec<ChatCompletionChunk>, ProviderError>;

    /// Health-Check
    async fn health_check(&self) -> bool;
}
