use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use egpu_manager_common::config::{AppRoutingConfig, LlmGatewayConfig, LlmProviderConfig};
use tokio::sync::{Mutex, RwLock};
use tracing::{debug, info, warn};

use super::budget::BudgetTracker;
use super::provider::{LlmProvider, ProviderError};
use super::providers::anthropic::AnthropicProvider;
use super::providers::gemini::GeminiProvider;
use super::providers::openai_compat::OpenAiCompatProvider;
use super::types::*;

/// Secrets loaded from /etc/egpu-manager/llm-secrets.toml
#[derive(Debug, Clone, Default, serde::Deserialize)]
pub struct LlmSecrets {
    #[serde(default)]
    pub keys: HashMap<String, String>,
}

impl LlmSecrets {
    pub fn load(path: &str) -> Self {
        match std::fs::read_to_string(path) {
            Ok(content) => toml::from_str(&content).unwrap_or_default(),
            Err(_) => {
                warn!("LLM-Secrets nicht ladbar: {path}");
                Self::default()
            }
        }
    }

    pub fn get(&self, key_ref: &str) -> Option<String> {
        self.keys.get(key_ref).cloned()
    }
}

/// Rate limiter using token bucket per app/provider
struct RateLimiter {
    /// (app_id, provider_name) -> (last_reset, count)
    counters: HashMap<(String, String), (Instant, u32)>,
    window: std::time::Duration,
}

impl RateLimiter {
    fn new() -> Self {
        Self {
            counters: HashMap::new(),
            window: std::time::Duration::from_secs(60),
        }
    }

    fn check_and_increment(&mut self, app_id: &str, provider: &str, limit: u32) -> bool {
        if limit == 0 {
            return true; // unlimited
        }

        let key = (app_id.to_string(), provider.to_string());
        let now = Instant::now();

        let (last_reset, count) = self
            .counters
            .entry(key)
            .or_insert((now, 0));

        if last_reset.elapsed() >= self.window {
            *last_reset = now;
            *count = 1;
            true
        } else if *count < limit {
            *count += 1;
            true
        } else {
            false
        }
    }
}

/// The LLM Gateway Router. Routes requests to the appropriate provider
/// based on app configuration, rate limits, and budget.
pub struct LlmRouter {
    providers: Vec<Arc<dyn LlmProvider>>,
    config: LlmGatewayConfig,
    app_routing: HashMap<String, AppRoutingConfig>,
    rate_limiter: Mutex<RateLimiter>,
    pub budget: Arc<Mutex<BudgetTracker>>,
    /// FIX 17: Gecachtes Health-Check-Ergebnis (Zeitpunkt, Ergebnisse)
    last_health_check: RwLock<(Instant, Vec<ProviderStatus>)>,
}

impl LlmRouter {
    /// Create a new LLM Router from config and secrets.
    pub fn new(config: LlmGatewayConfig, secrets: &LlmSecrets) -> Self {
        let mut providers: Vec<Arc<dyn LlmProvider>> = Vec::new();

        for provider_cfg in &config.providers {
            if !provider_cfg.enabled {
                continue;
            }

            let api_key = if provider_cfg.api_key_ref.is_empty() {
                None
            } else {
                secrets.get(&provider_cfg.api_key_ref)
            };

            let provider: Arc<dyn LlmProvider> = match provider_cfg.provider_type.as_str() {
                "anthropic" => Arc::new(AnthropicProvider::new(
                    provider_cfg.name.clone(),
                    api_key,
                    provider_cfg.models.clone(),
                )),
                "gemini" => Arc::new(GeminiProvider::new(
                    provider_cfg.name.clone(),
                    api_key,
                    provider_cfg.models.clone(),
                )),
                _ => Arc::new(OpenAiCompatProvider::new(
                    provider_cfg.name.clone(),
                    provider_cfg.base_url.clone(),
                    api_key,
                    provider_cfg.models.clone(),
                )),
            };

            info!("LLM Provider registriert: {} ({})", provider_cfg.name, provider_cfg.provider_type);
            providers.push(provider);
        }

        let app_routing: HashMap<String, AppRoutingConfig> = config
            .app_routing
            .iter()
            .map(|r| (r.app_id.clone(), r.clone()))
            .collect();

        Self {
            providers,
            config,
            app_routing,
            rate_limiter: Mutex::new(RateLimiter::new()),
            budget: Arc::new(Mutex::new(BudgetTracker::new())),
            last_health_check: RwLock::new((Instant::now() - std::time::Duration::from_secs(120), Vec::new())),
        }
    }

    /// Route a chat completion request.
    pub async fn chat_completion(
        &self,
        mut request: ChatCompletionRequest,
        app_id: &str,
    ) -> Result<ChatCompletionResponse, GatewayError> {
        request.app_id = Some(app_id.to_string());

        // Check app-level permissions
        let app_config = self.app_routing.get(app_id);

        // Check model allowed
        if let Some(cfg) = app_config {
            if !cfg.allowed_models.is_empty()
                && !cfg.allowed_models.iter().any(|m| m == &request.model)
            {
                return Err(GatewayError::model_not_allowed(&request.model));
            }
        }

        // Check budget
        {
            let budget = self.budget.lock().await;
            let app_limit = app_config.map(|c| c.monthly_budget_usd).unwrap_or(0.0);
            let limit = if app_limit > 0.0 {
                app_limit
            } else {
                self.config.monthly_budget_usd
            };

            if limit > 0.0 && budget.monthly_cost_for_app(app_id) >= limit {
                return Err(GatewayError::budget_exceeded());
            }
        }

        // Find provider
        let provider = self.select_provider(&request, app_id).await?;

        // Check rate limit
        {
            let mut limiter = self.rate_limiter.lock().await;
            let app_limit = app_config.map(|c| c.rate_limit_rpm).unwrap_or(0);
            let limit = if app_limit > 0 {
                app_limit
            } else {
                self.config.global_rate_limit_rpm
            };

            if !limiter.check_and_increment(app_id, provider.name(), limit) {
                return Err(GatewayError::rate_limited());
            }
        }

        // Execute request
        let start = Instant::now();
        let response = provider
            .chat_completion(&request)
            .await
            .map_err(|e| GatewayError::new(e.to_string(), "provider_error"))?;

        // Track usage
        if let Some(ref usage) = response.usage {
            let provider_cfg = self.find_provider_config(provider.name());
            let cost = self.calculate_cost(usage, provider_cfg);

            let record = UsageRecord {
                app_id: app_id.to_string(),
                provider: provider.name().to_string(),
                model: response.model.clone(),
                input_tokens: usage.prompt_tokens,
                output_tokens: usage.completion_tokens,
                cost_usd: cost,
                timestamp: chrono::Utc::now(),
                request_id: response.id.clone(),
                duration_ms: start.elapsed().as_millis() as u64,
            };

            let mut budget = self.budget.lock().await;
            budget.record_usage(record);
        }

        Ok(response)
    }

    /// Select the best provider for a request.
    async fn select_provider(
        &self,
        request: &ChatCompletionRequest,
        app_id: &str,
    ) -> Result<Arc<dyn LlmProvider>, GatewayError> {
        // If client explicitly requested a provider
        if let Some(ref provider_name) = request.provider {
            return self
                .providers
                .iter()
                .find(|p| p.name() == provider_name && p.supports_model(&request.model))
                .cloned()
                .ok_or_else(|| GatewayError::provider_unavailable(provider_name));
        }

        // Check app routing preferences
        let preferred = self
            .app_routing
            .get(app_id)
            .map(|c| &c.preferred_provider)
            .filter(|p| !p.is_empty());

        let allowed = self
            .app_routing
            .get(app_id)
            .map(|c| &c.allowed_providers)
            .filter(|p| !p.is_empty());

        // Try preferred provider first
        if let Some(pref) = preferred {
            if let Some(p) = self
                .providers
                .iter()
                .find(|p| p.name() == pref && p.supports_model(&request.model))
            {
                return Ok(Arc::clone(p));
            }
        }

        // Try any allowed provider that supports the model (sorted by config priority)
        let mut candidates: Vec<_> = self
            .providers
            .iter()
            .filter(|p| p.supports_model(&request.model))
            .filter(|p| {
                allowed
                    .map(|a| a.iter().any(|name| name == p.name()))
                    .unwrap_or(true)
            })
            .collect();

        // Sort by config priority
        candidates.sort_by_key(|p| {
            self.find_provider_config(p.name())
                .map(|c| c.priority)
                .unwrap_or(99)
        });

        candidates
            .first()
            .map(|p| Arc::clone(p))
            .ok_or_else(GatewayError::no_provider)
    }

    fn find_provider_config(&self, name: &str) -> Option<&LlmProviderConfig> {
        self.config.providers.iter().find(|p| p.name == name)
    }

    fn calculate_cost(&self, usage: &TokenUsage, provider_cfg: Option<&LlmProviderConfig>) -> f64 {
        let Some(cfg) = provider_cfg else {
            return 0.0;
        };
        let input_cost = (usage.prompt_tokens as f64 / 1_000_000.0) * cfg.cost_per_1m_input_tokens;
        let output_cost =
            (usage.completion_tokens as f64 / 1_000_000.0) * cfg.cost_per_1m_output_tokens;
        input_cost + output_cost
    }

    /// Get provider status list.
    /// FIX 17: Health-Check-Ergebnisse werden 60 Sekunden gecacht.
    pub async fn provider_status(&self) -> Vec<ProviderStatus> {
        let health_cache_duration = std::time::Duration::from_secs(60);

        // Pruefen ob Cache noch gueltig ist
        {
            let cache = self.last_health_check.read().await;
            if cache.0.elapsed() < health_cache_duration && !cache.1.is_empty() {
                return cache.1.clone();
            }
        }

        // Cache abgelaufen — Health-Checks ausfuehren
        let budget = self.budget.lock().await;
        let mut statuses = Vec::new();

        for provider in &self.providers {
            let cfg = self.find_provider_config(provider.name());
            let (requests, cost) = budget.daily_stats_for_provider(provider.name());
            let healthy = provider.health_check().await;

            statuses.push(ProviderStatus {
                name: provider.name().to_string(),
                provider_type: cfg
                    .map(|c| c.provider_type.clone())
                    .unwrap_or_default(),
                enabled: cfg.map(|c| c.enabled).unwrap_or(true),
                healthy,
                models: cfg.map(|c| c.models.clone()).unwrap_or_default(),
                requests_today: requests,
                cost_today_usd: cost,
            });
        }

        // Cache aktualisieren
        {
            let mut cache = self.last_health_check.write().await;
            *cache = (Instant::now(), statuses.clone());
        }

        statuses
    }

    /// Get usage summary for an app.
    pub async fn usage_for_app(&self, app_id: &str) -> AppUsageSummary {
        let budget = self.budget.lock().await;
        budget.summary_for_app(app_id)
    }
}

/// App usage summary returned by the API.
#[derive(Debug, Clone, serde::Serialize)]
pub struct AppUsageSummary {
    pub app_id: String,
    pub total_requests: u64,
    pub total_input_tokens: u64,
    pub total_output_tokens: u64,
    pub total_cost_usd: f64,
    pub month_cost_usd: f64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rate_limiter_allows_under_limit() {
        let mut limiter = RateLimiter::new();
        assert!(limiter.check_and_increment("app1", "ollama", 10));
        assert!(limiter.check_and_increment("app1", "ollama", 10));
    }

    #[test]
    fn test_rate_limiter_blocks_over_limit() {
        let mut limiter = RateLimiter::new();
        for _ in 0..5 {
            assert!(limiter.check_and_increment("app1", "ollama", 5));
        }
        assert!(!limiter.check_and_increment("app1", "ollama", 5));
    }

    #[test]
    fn test_rate_limiter_unlimited() {
        let mut limiter = RateLimiter::new();
        for _ in 0..1000 {
            assert!(limiter.check_and_increment("app1", "ollama", 0));
        }
    }

    #[test]
    fn test_rate_limiter_separate_apps() {
        let mut limiter = RateLimiter::new();
        for _ in 0..5 {
            limiter.check_and_increment("app1", "ollama", 5);
        }
        // app2 should still be allowed
        assert!(limiter.check_and_increment("app2", "ollama", 5));
    }

    #[test]
    fn test_secrets_load_missing_file() {
        let secrets = LlmSecrets::load("/nonexistent/file.toml");
        assert!(secrets.keys.is_empty());
    }

    #[test]
    fn test_cost_calculation() {
        let config = LlmGatewayConfig::default();
        let secrets = LlmSecrets::default();
        let router = LlmRouter::new(config, &secrets);

        let usage = TokenUsage {
            prompt_tokens: 1000,
            completion_tokens: 500,
            total_tokens: 1500,
        };

        let provider_cfg = LlmProviderConfig {
            name: "test".into(),
            provider_type: "openai_compatible".into(),
            base_url: "http://localhost".into(),
            api_key_ref: String::new(),
            models: vec![],
            rate_limit_rpm: 0,
            enabled: true,
            priority: 1,
            cost_per_1m_input_tokens: 3.0,
            cost_per_1m_output_tokens: 15.0,
        };

        let cost = router.calculate_cost(&usage, Some(&provider_cfg));
        // 1000/1M * 3.0 + 500/1M * 15.0 = 0.003 + 0.0075 = 0.0105
        assert!((cost - 0.0105).abs() < 0.0001);
    }
}
