use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use tracing::warn;

use super::router::LlmRouter;
use super::types::*;

/// Shared LLM Gateway state for Axum handlers.
pub type LlmState = Arc<LlmRouter>;

/// Create the LLM API router with all /api/llm/* endpoints.
pub fn llm_api_routes() -> Router<LlmState> {
    Router::new()
        .route("/api/llm/chat/completions", post(chat_completions))
        .route("/api/llm/providers", get(list_providers))
        .route("/api/llm/usage/{app_id}", get(get_usage))
        .route("/api/llm/health", get(health_check))
}

/// POST /api/llm/chat/completions
/// OpenAI-compatible chat completion endpoint.
/// Requires X-App-Id header for routing.
async fn chat_completions(
    State(router): State<LlmState>,
    headers: axum::http::HeaderMap,
    Json(request): Json<ChatCompletionRequest>,
) -> impl IntoResponse {
    let app_id = headers
        .get("x-app-id")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("unknown");

    match router.chat_completion(request, app_id).await {
        Ok(response) => (StatusCode::OK, Json(serde_json::to_value(response).unwrap())),
        Err(err) => {
            let status = match err.error.r#type.as_str() {
                "rate_limit_error" => StatusCode::TOO_MANY_REQUESTS,
                "budget_exceeded" => StatusCode::PAYMENT_REQUIRED,
                "permission_error" => StatusCode::FORBIDDEN,
                "routing_error" => StatusCode::SERVICE_UNAVAILABLE,
                _ => StatusCode::BAD_GATEWAY,
            };
            warn!("LLM Gateway Fehler: {} (App: {})", err.error.message, app_id);
            (status, Json(serde_json::to_value(err).unwrap()))
        }
    }
}

/// GET /api/llm/providers
/// List all configured providers and their status.
async fn list_providers(State(router): State<LlmState>) -> impl IntoResponse {
    let statuses = router.provider_status().await;
    Json(serde_json::json!({ "providers": statuses }))
}

/// GET /api/llm/usage/:app_id
/// Get usage statistics for an app.
async fn get_usage(
    State(router): State<LlmState>,
    Path(app_id): Path<String>,
) -> impl IntoResponse {
    let summary = router.usage_for_app(&app_id).await;
    Json(serde_json::to_value(summary).unwrap())
}

/// GET /api/llm/health
/// Health check for the LLM Gateway.
async fn health_check(State(router): State<LlmState>) -> impl IntoResponse {
    let providers = router.provider_status().await;
    let any_healthy = providers.iter().any(|p| p.healthy);

    let status = if any_healthy {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };

    (
        status,
        Json(serde_json::json!({
            "status": if any_healthy { "ok" } else { "degraded" },
            "providers_count": providers.len(),
            "healthy_count": providers.iter().filter(|p| p.healthy).count(),
        })),
    )
}
