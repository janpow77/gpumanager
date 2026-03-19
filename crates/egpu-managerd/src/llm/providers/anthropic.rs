use async_trait::async_trait;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use tracing::debug;

use crate::llm::provider::{LlmProvider, ProviderError};
use crate::llm::types::*;

/// Provider fuer die Anthropic Messages API
/// Konvertiert intern zwischen OpenAI-Format und Anthropic-Format
pub struct AnthropicProvider {
    name: String,
    api_key: Option<String>,
    models: Vec<String>,
    client: Client,
}

impl AnthropicProvider {
    const BASE_URL: &'static str = "https://api.anthropic.com";
    const API_VERSION: &'static str = "2023-06-01";

    pub fn new(name: String, api_key: Option<String>, models: Vec<String>) -> Self {
        let client = Client::builder()
            .timeout(std::time::Duration::from_secs(120))
            .build()
            .unwrap_or_else(|_| Client::new());

        Self {
            name,
            api_key,
            models,
            client,
        }
    }

    /// OpenAI-Format-Messages in Anthropic-Format konvertieren
    fn convert_request(&self, request: &ChatCompletionRequest) -> AnthropicRequest {
        let mut system = String::new();
        let mut messages = Vec::new();

        for msg in &request.messages {
            if msg.role == "system" {
                system = msg.content.clone();
            } else {
                messages.push(AnthropicMessage {
                    role: msg.role.clone(),
                    content: msg.content.clone(),
                });
            }
        }

        AnthropicRequest {
            model: request.model.clone(),
            messages,
            system: if system.is_empty() {
                None
            } else {
                Some(system)
            },
            max_tokens: request.max_tokens.unwrap_or(4096),
            temperature: request.temperature,
            top_p: request.top_p,
            stream: false,
        }
    }

    /// Anthropic-Antwort in OpenAI-Format konvertieren
    fn convert_response(&self, resp: AnthropicResponse) -> ChatCompletionResponse {
        let content = resp
            .content
            .into_iter()
            .filter_map(|c| {
                if c.r#type == "text" {
                    Some(c.text)
                } else {
                    None
                }
            })
            .collect::<Vec<_>>()
            .join("");

        ChatCompletionResponse {
            id: resp.id,
            object: "chat.completion".into(),
            created: chrono::Utc::now().timestamp(),
            model: resp.model,
            choices: vec![ChatChoice {
                index: 0,
                message: ChatMessage {
                    role: "assistant".into(),
                    content,
                },
                finish_reason: Some(resp.stop_reason.unwrap_or_else(|| "stop".into())),
            }],
            usage: Some(TokenUsage {
                prompt_tokens: resp.usage.input_tokens,
                completion_tokens: resp.usage.output_tokens,
                total_tokens: resp.usage.input_tokens + resp.usage.output_tokens,
            }),
            provider: self.name.clone(),
        }
    }
}

#[derive(Serialize)]
struct AnthropicRequest {
    model: String,
    messages: Vec<AnthropicMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    system: Option<String>,
    max_tokens: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    top_p: Option<f64>,
    stream: bool,
}

#[derive(Serialize, Deserialize)]
struct AnthropicMessage {
    role: String,
    content: String,
}

#[derive(Deserialize)]
struct AnthropicResponse {
    id: String,
    model: String,
    content: Vec<AnthropicContent>,
    stop_reason: Option<String>,
    usage: AnthropicUsage,
}

#[derive(Deserialize)]
struct AnthropicContent {
    r#type: String,
    text: String,
}

#[derive(Deserialize)]
struct AnthropicUsage {
    input_tokens: u32,
    output_tokens: u32,
}

#[async_trait]
impl LlmProvider for AnthropicProvider {
    fn name(&self) -> &str {
        &self.name
    }

    fn supports_model(&self, model: &str) -> bool {
        self.models.is_empty() || self.models.iter().any(|m| m == model)
    }

    async fn chat_completion(
        &self,
        request: &ChatCompletionRequest,
    ) -> Result<ChatCompletionResponse, ProviderError> {
        debug!("Anthropic request model={}", request.model);

        let anthropic_req = self.convert_request(request);
        let url = format!("{}/v1/messages", Self::BASE_URL);

        let mut req = self
            .client
            .post(&url)
            .json(&anthropic_req)
            .header("anthropic-version", Self::API_VERSION)
            .header("content-type", "application/json");

        if let Some(ref key) = self.api_key {
            req = req.header("x-api-key", key);
        }

        let resp = req.send().await?;
        let status = resp.status().as_u16();

        if !resp.status().is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(ProviderError::Api {
                status,
                message: body,
            });
        }

        let anthropic_resp: AnthropicResponse = resp.json().await?;
        Ok(self.convert_response(anthropic_resp))
    }

    async fn health_check(&self) -> bool {
        // Anthropic hat keinen einfachen Health-Endpoint;
        // pruefe ob ein API-Key konfiguriert ist
        self.api_key.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_convert_request_with_system() {
        let provider = AnthropicProvider::new("test".into(), None, vec![]);
        let request = ChatCompletionRequest {
            model: "claude-sonnet-4-20250514".into(),
            messages: vec![
                ChatMessage {
                    role: "system".into(),
                    content: "You are helpful".into(),
                },
                ChatMessage {
                    role: "user".into(),
                    content: "Hello".into(),
                },
            ],
            temperature: Some(0.7),
            max_tokens: Some(1000),
            stream: false,
            top_p: None,
            stop: None,
            app_id: None,
            provider: None,
            workload_type: None,
        };

        let converted = provider.convert_request(&request);
        assert_eq!(converted.system, Some("You are helpful".into()));
        assert_eq!(converted.messages.len(), 1); // system wurde extrahiert
        assert_eq!(converted.messages[0].role, "user");
        assert_eq!(converted.max_tokens, 1000);
    }
}
