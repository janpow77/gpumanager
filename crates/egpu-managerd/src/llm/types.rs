use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Unified chat completion request (OpenAI-compatible format)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatCompletionRequest {
    pub model: String,
    pub messages: Vec<ChatMessage>,
    #[serde(default)]
    pub temperature: Option<f64>,
    #[serde(default)]
    pub max_tokens: Option<u32>,
    #[serde(default)]
    pub stream: bool,
    #[serde(default)]
    pub top_p: Option<f64>,
    #[serde(default)]
    pub stop: Option<Vec<String>>,
    /// App ID for routing (set by gateway, not by client)
    #[serde(skip_deserializing)]
    pub app_id: Option<String>,
    /// Provider override (client can request specific provider)
    #[serde(default)]
    pub provider: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: String,
    pub content: String,
}

/// Unified chat completion response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatCompletionResponse {
    pub id: String,
    pub object: String,
    pub created: i64,
    pub model: String,
    pub choices: Vec<ChatChoice>,
    pub usage: Option<TokenUsage>,
    /// Which provider actually served this request
    #[serde(default)]
    pub provider: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatChoice {
    pub index: u32,
    pub message: ChatMessage,
    pub finish_reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenUsage {
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    pub total_tokens: u32,
}

/// Streaming chunk (SSE format)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatCompletionChunk {
    pub id: String,
    pub object: String,
    pub created: i64,
    pub model: String,
    pub choices: Vec<ChunkChoice>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChunkChoice {
    pub index: u32,
    pub delta: ChunkDelta,
    pub finish_reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChunkDelta {
    #[serde(default)]
    pub role: Option<String>,
    #[serde(default)]
    pub content: Option<String>,
}

/// Provider status info
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderStatus {
    pub name: String,
    pub provider_type: String,
    pub enabled: bool,
    pub healthy: bool,
    pub models: Vec<String>,
    pub requests_today: u64,
    pub cost_today_usd: f64,
}

/// Usage record for budget tracking
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UsageRecord {
    pub app_id: String,
    pub provider: String,
    pub model: String,
    pub input_tokens: u32,
    pub output_tokens: u32,
    pub cost_usd: f64,
    pub timestamp: DateTime<Utc>,
    pub request_id: String,
    pub duration_ms: u64,
}

/// Gateway error response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GatewayError {
    pub error: GatewayErrorDetail,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GatewayErrorDetail {
    pub message: String,
    pub r#type: String,
    pub code: Option<String>,
}

impl GatewayError {
    pub fn new(message: impl Into<String>, error_type: impl Into<String>) -> Self {
        Self {
            error: GatewayErrorDetail {
                message: message.into(),
                r#type: error_type.into(),
                code: None,
            },
        }
    }

    pub fn rate_limited() -> Self {
        Self::new("Rate limit exceeded", "rate_limit_error")
    }

    pub fn budget_exceeded() -> Self {
        Self::new("Monthly budget exceeded", "budget_exceeded")
    }

    pub fn provider_unavailable(provider: &str) -> Self {
        Self::new(
            format!("Provider '{provider}' unavailable"),
            "provider_error",
        )
    }

    pub fn no_provider() -> Self {
        Self::new("No suitable provider found", "routing_error")
    }

    pub fn model_not_allowed(model: &str) -> Self {
        Self::new(
            format!("Model '{model}' not allowed for this app"),
            "permission_error",
        )
    }
}
