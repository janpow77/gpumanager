use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use egpu_manager_common::config::{AppRoutingConfig, LlmGatewayConfig, LlmProviderConfig};
use tokio::sync::{Mutex, RwLock};
use tracing::{info, warn};

use super::budget::BudgetTracker;
use super::provider::LlmProvider;
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
/// Mit GPU-Aware Routing: kennt MonitorState und OllamaFleet.
pub struct LlmRouter {
    providers: Vec<Arc<dyn LlmProvider>>,
    config: LlmGatewayConfig,
    app_routing: HashMap<String, AppRoutingConfig>,
    rate_limiter: Mutex<RateLimiter>,
    pub budget: Arc<Mutex<BudgetTracker>>,
    /// FIX 17: Gecachtes Health-Check-Ergebnis (Zeitpunkt, Ergebnisse)
    last_health_check: RwLock<(Instant, Vec<ProviderStatus>)>,
    /// Shared MonitorState für GPU-Aware Routing (optional für backward-compat)
    monitor_state: Option<Arc<Mutex<crate::monitor::MonitorState>>>,
    /// OllamaFleet für Multi-Instanz-Routing (optional)
    ollama_fleet: Option<Arc<crate::ollama::OllamaFleet>>,
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
            monitor_state: None,
            ollama_fleet: None,
        }
    }

    /// Setzt den MonitorState für GPU-Aware Routing.
    pub fn set_monitor_state(&mut self, state: Arc<Mutex<crate::monitor::MonitorState>>) {
        self.monitor_state = Some(state);
    }

    /// Setzt die OllamaFleet für Multi-Instanz-Routing.
    pub fn set_ollama_fleet(&mut self, fleet: Arc<crate::ollama::OllamaFleet>) {
        self.ollama_fleet = Some(fleet);
    }

    /// Route a chat completion request.
    /// Mit GPU-Aware Routing: Workload-Typ → Modell → Instanz → VRAM-Check → Preemption.
    pub async fn chat_completion(
        &self,
        mut request: ChatCompletionRequest,
        app_id: &str,
    ) -> Result<ChatCompletionResponse, GatewayError> {
        request.app_id = Some(app_id.to_string());

        // GPU-Aware: Modell aus Workload-Typ auflösen
        let (resolved_model, resolved_instance) = self.resolve_model_from_workload(&request, app_id);
        if request.model != resolved_model {
            info!(
                "GPU-Aware Routing: Modell '{}' -> '{}' (Workload: {:?}, App: {})",
                request.model, resolved_model, request.workload_type, app_id
            );
            request.model = resolved_model;
        }

        // Provider-Override aus Fleet-Instanz
        if request.provider.is_none() {
            if let Some(ref inst_name) = resolved_instance {
                request.provider = Some(inst_name.clone());
            }
        }

        // Check app-level permissions
        let app_config = self.app_routing.get(app_id);

        // Check model allowed (erweitert: workload_model_map Modelle sind implizit erlaubt)
        if let Some(cfg) = app_config {
            let model_from_map = cfg.workload_model_map.values().any(|m| m == &request.model);
            if !model_from_map
                && !cfg.allowed_models.is_empty()
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

        // GPU-Aware: VRAM-Preemption falls nötig
        if let Some(ref inst_name) = resolved_instance {
            if let Some(ref fleet) = self.ollama_fleet {
                if let Some(inst) = fleet.instance_by_name(inst_name) {
                    // Prüfe ob genug VRAM verfügbar ist
                    if let Some(ref monitor) = self.monitor_state {
                        let (vram_available, inst_priority) = {
                            let st = monitor.lock().await;
                            (
                                st.scheduler.vram_available(inst.gpu_target),
                                inst.config.priority,
                            )
                        };

                        // Typischer VRAM-Bedarf aus workload_type Config
                        let needed_vram = self.config.app_routing.iter()
                            .find(|r| r.app_id == app_id)
                            .and_then(|_| {
                                request.workload_type.as_deref()
                            })
                            .and_then(|_wt| {
                                // Konservativer Default: 2 GB für Embedding, 10 GB für LLM
                                match request.workload_type.as_deref() {
                                    Some("embeddings") => Some(2000u64),
                                    Some("llm") => Some(10000u64),
                                    _ => None,
                                }
                            })
                            .unwrap_or(0);

                        if needed_vram > 0 && vram_available < needed_vram {
                            let preempted = self.preempt_models_if_needed(
                                inst.gpu_target,
                                needed_vram,
                                inst_priority,
                            ).await;
                            if !preempted.is_empty() {
                                info!(
                                    "Preemption vor Request: {} Modell(e) entladen auf {:?}",
                                    preempted.len(), inst.gpu_target
                                );
                            }
                        }
                    }
                }
            }
        }

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

    /// GPU-Aware Modell-Auflösung: Bestimmt Modell und Provider aus App-Config + Workload-Typ.
    fn resolve_model_from_workload(
        &self,
        request: &ChatCompletionRequest,
        app_id: &str,
    ) -> (String, Option<String>) {
        let workload_type = request.workload_type.as_deref();
        let app_config = self.app_routing.get(app_id);

        // Wenn ein Workload-Typ angegeben ist und die App ein workload_model_map hat,
        // das Modell daraus auflösen
        if let Some(wt) = workload_type {
            if let Some(cfg) = app_config {
                if let Some(model) = cfg.workload_model_map.get(wt) {
                    // Auch passende Instanz finden
                    if let Some(ref fleet) = self.ollama_fleet {
                        if let Some(instance) = fleet.instance_for_model(model) {
                            return (model.clone(), Some(instance.config.name.clone()));
                        }
                        // Fallback: Instanz über Workload-Type finden
                        let instances = fleet.instances_for_workload(wt);
                        if let Some(inst) = instances.first() {
                            return (model.clone(), Some(inst.config.name.clone()));
                        }
                    }
                    return (model.clone(), None);
                }
            }
        }

        // Kein Workload-Mapping: Modell aus Request verwenden
        (request.model.clone(), None)
    }

    /// Automatische Model-Preemption: Entlädt niedrig-priore Modelle um Platz zu schaffen.
    async fn preempt_models_if_needed(
        &self,
        target_gpu: crate::scheduler::GpuTarget,
        needed_vram_mb: u64,
        requesting_priority: u32,
    ) -> Vec<String> {
        let mut preempted = Vec::new();

        let Some(ref monitor) = self.monitor_state else {
            return preempted;
        };
        let Some(ref fleet) = self.ollama_fleet else {
            return preempted;
        };

        // Finde preemptable Modelle im Scheduler
        let candidates = {
            let st = monitor.lock().await;
            st.scheduler.find_preemptable_models(target_gpu, needed_vram_mb, requesting_priority)
        };

        if candidates.is_empty() {
            return preempted;
        }

        for (model_name, instance_name, vram_mb) in &candidates {
            info!(
                "Preemption: Entlade '{}' von '{}' ({} MB VRAM, GPU {:?})",
                model_name, instance_name, vram_mb, target_gpu
            );

            if let Err(e) = fleet.unload_model_on(instance_name, model_name).await {
                warn!(
                    "Preemption fehlgeschlagen für '{}' auf '{}': {}",
                    model_name, instance_name, e
                );
                continue;
            }

            // Aus Scheduler entfernen
            {
                let mut st = monitor.lock().await;
                st.scheduler.unregister_model(model_name, instance_name);
            }

            preempted.push(model_name.clone());
        }

        preempted
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

    /// Proxy-Embedding-Request: Löst Modell + Instanz auf und leitet an die richtige
    /// Ollama-Instanz weiter (/api/embed).
    pub async fn embed(
        &self,
        mut request: EmbeddingRequest,
        app_id: &str,
    ) -> Result<EmbeddingResponse, GatewayError> {
        // Workload-Typ ist immer "embeddings" für Embedding-Requests
        let workload_type = request.workload_type.clone().unwrap_or_else(|| "embeddings".to_string());

        // Modell aus App-Config auflösen
        if let Some(cfg) = self.app_routing.get(app_id) {
            if let Some(model) = cfg.workload_model_map.get(&workload_type) {
                if request.model == "auto" || request.model.is_empty() {
                    request.model = model.clone();
                }
            }
        }

        // Ollama-Host finden: Fleet hat Vorrang
        let ollama_host = if let Some(ref fleet) = self.ollama_fleet {
            // Zuerst: Instanz die das Modell hat
            if let Some(inst) = fleet.instance_for_model(&request.model) {
                inst.config.host.clone()
            }
            // Dann: Instanz für den Workload-Typ
            else if let Some(inst) = fleet.instances_for_workload(&workload_type).first() {
                inst.config.host.clone()
            } else {
                return Err(GatewayError::new(
                    format!("Keine Ollama-Instanz für Modell '{}' / Workload '{}'", request.model, workload_type),
                    "routing_error",
                ));
            }
        } else {
            // Legacy: erster Provider mit openai_compatible
            self.config.providers.iter()
                .find(|p| p.provider_type == "openai_compatible" && p.enabled)
                .map(|p| p.base_url.clone())
                .ok_or_else(GatewayError::no_provider)?
        };

        // Preemption falls nötig
        if let Some(ref fleet) = self.ollama_fleet {
            if let Some(inst) = fleet.instance_for_model(&request.model) {
                if let Some(ref monitor) = self.monitor_state {
                    let vram_available = {
                        let st = monitor.lock().await;
                        st.scheduler.vram_available(inst.gpu_target)
                    };
                    if vram_available < 2000 {
                        let preempted = self.preempt_models_if_needed(
                            inst.gpu_target, 2000, inst.config.priority,
                        ).await;
                        if !preempted.is_empty() {
                            info!("Embedding-Preemption: {} Modell(e) entladen", preempted.len());
                        }
                    }
                }
            }
        }

        // Request an Ollama /api/embed weiterleiten
        let url = format!("{}/api/embed", ollama_host);
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(120))
            .build()
            .map_err(|e| GatewayError::new(e.to_string(), "provider_error"))?;

        let resp = client
            .post(&url)
            .json(&serde_json::json!({
                "model": request.model,
                "input": request.input,
            }))
            .send()
            .await
            .map_err(|e| GatewayError::new(
                format!("Ollama nicht erreichbar ({}): {}", ollama_host, e),
                "provider_error",
            ))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(GatewayError::new(
                format!("Ollama Embedding-Fehler: HTTP {} — {}", status, body),
                "provider_error",
            ));
        }

        resp.json::<EmbeddingResponse>()
            .await
            .map_err(|e| GatewayError::new(
                format!("Ollama Embedding-Response ungültig: {}", e),
                "provider_error",
            ))
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
