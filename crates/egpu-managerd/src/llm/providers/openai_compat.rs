use async_trait::async_trait;
use reqwest::Client;
use tracing::{debug, warn};

use crate::llm::provider::{LlmProvider, ProviderError};
use crate::llm::types::*;

/// OpenAI-kompatibler Provider (funktioniert mit Ollama, xAI/Grok, DeepSeek, Zhipu
/// und jeder anderen OpenAI-kompatiblen API)
pub struct OpenAiCompatProvider {
    name: String,
    base_url: String,
    api_key: Option<String>,
    models: Vec<String>,
    client: Client,
}

impl OpenAiCompatProvider {
    pub fn new(
        name: String,
        base_url: String,
        api_key: Option<String>,
        models: Vec<String>,
    ) -> Self {
        let client = Client::builder()
            .timeout(std::time::Duration::from_secs(120))
            .build()
            .unwrap_or_else(|_| Client::new());

        Self {
            name,
            base_url: base_url.trim_end_matches('/').to_string(),
            api_key,
            models,
            client,
        }
    }

    fn build_request(&self, request: &ChatCompletionRequest) -> reqwest::RequestBuilder {
        let url = format!("{}/v1/chat/completions", self.base_url);
        let mut req = self.client.post(&url).json(request);
        if let Some(ref key) = self.api_key {
            req = req.bearer_auth(key);
        }
        req
    }
}

#[async_trait]
impl LlmProvider for OpenAiCompatProvider {
    fn name(&self) -> &str {
        &self.name
    }

    fn supports_model(&self, model: &str) -> bool {
        // Leere Modell-Liste bedeutet: alle Modelle werden akzeptiert
        self.models.is_empty() || self.models.iter().any(|m| m == model)
    }

    async fn chat_completion(
        &self,
        request: &ChatCompletionRequest,
    ) -> Result<ChatCompletionResponse, ProviderError> {
        debug!("OpenAI-compat request to {} model={}", self.name, request.model);

        let resp = self.build_request(request).send().await?;
        let status = resp.status().as_u16();

        if !resp.status().is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(ProviderError::Api {
                status,
                message: body,
            });
        }

        let mut response: ChatCompletionResponse = resp.json().await?;
        response.provider = self.name.clone();
        Ok(response)
    }

    async fn health_check(&self) -> bool {
        let url = format!("{}/v1/models", self.base_url);
        let mut req = self.client.get(&url);
        if let Some(ref key) = self.api_key {
            req = req.bearer_auth(key);
        }
        match req.timeout(std::time::Duration::from_secs(5)).send().await {
            Ok(resp) => resp.status().is_success(),
            Err(e) => {
                warn!("Health check failed for {}: {}", self.name, e);
                false
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_supports_model_empty_list() {
        let provider = OpenAiCompatProvider::new(
            "test".into(),
            "http://localhost:11434".into(),
            None,
            vec![],
        );
        assert!(provider.supports_model("any-model"));
    }

    #[test]
    fn test_supports_model_specific_list() {
        let provider = OpenAiCompatProvider::new(
            "test".into(),
            "http://localhost:11434".into(),
            None,
            vec!["qwen3:14b".into(), "llama3:8b".into()],
        );
        assert!(provider.supports_model("qwen3:14b"));
        assert!(!provider.supports_model("gpt-4"));
    }

    #[test]
    fn test_base_url_trailing_slash_stripped() {
        let provider = OpenAiCompatProvider::new(
            "test".into(),
            "http://localhost:11434/".into(),
            None,
            vec![],
        );
        assert_eq!(provider.base_url, "http://localhost:11434");
    }
}
