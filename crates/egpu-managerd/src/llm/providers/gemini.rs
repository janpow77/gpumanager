use async_trait::async_trait;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use tracing::debug;

use crate::llm::provider::{LlmProvider, ProviderError};
use crate::llm::types::*;

/// Provider fuer die Google Gemini API
/// Konvertiert intern zwischen OpenAI-Format und Gemini-Format
pub struct GeminiProvider {
    name: String,
    api_key: Option<String>,
    models: Vec<String>,
    client: Client,
}

impl GeminiProvider {
    const BASE_URL: &'static str = "https://generativelanguage.googleapis.com/v1beta";

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

    /// OpenAI-Format-Request in Gemini-Format konvertieren
    fn convert_request(&self, request: &ChatCompletionRequest) -> GeminiRequest {
        let mut system_instruction = None;
        let mut contents = Vec::new();

        for msg in &request.messages {
            if msg.role == "system" {
                system_instruction = Some(GeminiContent {
                    role: None,
                    parts: vec![GeminiPart {
                        text: msg.content.clone(),
                    }],
                });
            } else {
                // Gemini verwendet "model" statt "assistant"
                let role = match msg.role.as_str() {
                    "assistant" => "model",
                    _ => "user",
                };
                contents.push(GeminiContent {
                    role: Some(role.into()),
                    parts: vec![GeminiPart {
                        text: msg.content.clone(),
                    }],
                });
            }
        }

        let generation_config = Some(GeminiGenerationConfig {
            temperature: request.temperature,
            top_p: request.top_p,
            max_output_tokens: request.max_tokens,
        });

        GeminiRequest {
            contents,
            system_instruction,
            generation_config,
        }
    }

    /// Gemini-Antwort in OpenAI-Format konvertieren
    fn convert_response(&self, resp: GeminiResponse, model: &str) -> ChatCompletionResponse {
        let content = resp
            .candidates
            .first()
            .and_then(|c| c.content.parts.first())
            .map(|p| p.text.clone())
            .unwrap_or_default();

        let usage = resp.usage_metadata.map(|u| TokenUsage {
            prompt_tokens: u.prompt_token_count,
            completion_tokens: u.candidates_token_count,
            total_tokens: u.total_token_count,
        });

        ChatCompletionResponse {
            id: format!("gemini-{}", uuid::Uuid::new_v4()),
            object: "chat.completion".into(),
            created: chrono::Utc::now().timestamp(),
            model: model.into(),
            choices: vec![ChatChoice {
                index: 0,
                message: ChatMessage {
                    role: "assistant".into(),
                    content,
                },
                finish_reason: Some("stop".into()),
            }],
            usage,
            provider: self.name.clone(),
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct GeminiRequest {
    contents: Vec<GeminiContent>,
    #[serde(skip_serializing_if = "Option::is_none")]
    system_instruction: Option<GeminiContent>,
    #[serde(skip_serializing_if = "Option::is_none")]
    generation_config: Option<GeminiGenerationConfig>,
}

#[derive(Serialize, Deserialize)]
struct GeminiContent {
    #[serde(skip_serializing_if = "Option::is_none")]
    role: Option<String>,
    parts: Vec<GeminiPart>,
}

#[derive(Serialize, Deserialize)]
struct GeminiPart {
    text: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct GeminiGenerationConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    top_p: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_output_tokens: Option<u32>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GeminiResponse {
    candidates: Vec<GeminiCandidate>,
    usage_metadata: Option<GeminiUsageMetadata>,
}

#[derive(Deserialize)]
struct GeminiCandidate {
    content: GeminiContent,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GeminiUsageMetadata {
    prompt_token_count: u32,
    candidates_token_count: u32,
    total_token_count: u32,
}

#[async_trait]
impl LlmProvider for GeminiProvider {
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
        debug!("Gemini request model={}", request.model);

        let gemini_req = self.convert_request(request);
        let url = format!(
            "{}/models/{}:generateContent?key={}",
            Self::BASE_URL,
            request.model,
            self.api_key.as_deref().unwrap_or("")
        );

        let resp = self.client.post(&url).json(&gemini_req).send().await?;
        let status = resp.status().as_u16();

        if !resp.status().is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(ProviderError::Api {
                status,
                message: body,
            });
        }

        let gemini_resp: GeminiResponse = resp.json().await?;
        Ok(self.convert_response(gemini_resp, &request.model))
    }

    async fn health_check(&self) -> bool {
        // Pruefe ob ein API-Key konfiguriert ist
        self.api_key.is_some()
    }
}
