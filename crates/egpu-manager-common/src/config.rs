use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub schema_version: u32,

    #[serde(default)]
    pub system: SystemConfig,

    #[serde(default)]
    pub database: DatabaseConfig,

    pub gpu: GpuConfig,

    #[serde(default)]
    pub thunderbolt: Option<ThunderboltConfig>,

    #[serde(default)]
    pub docker: DockerConfig,

    #[serde(default)]
    pub local_api: LocalApiConfig,

    #[serde(default)]
    pub remote: Option<RemoteConfig>,

    #[serde(default)]
    pub ollama: Option<OllamaConfig>,

    #[serde(default)]
    pub notifications: NotificationsConfig,

    #[serde(default)]
    pub recovery: RecoveryConfig,

    #[serde(default)]
    pub daemon: DaemonConfig,

    #[serde(default)]
    pub pipeline: Vec<PipelineConfig>,

    #[serde(default)]
    pub remote_gpu: Vec<RemoteGpuConfig>,

    #[serde(default)]
    pub llm_gateway: Option<LlmGatewayConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SystemConfig {
    #[serde(default = "default_log_level")]
    pub log_level: String,
}

impl Default for SystemConfig {
    fn default() -> Self {
        Self {
            log_level: default_log_level(),
        }
    }
}

fn default_log_level() -> String {
    "info".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DatabaseConfig {
    #[serde(default = "default_db_path")]
    pub db_path: String,
    #[serde(default = "default_retention_days")]
    pub retention_days: u32,
    #[serde(default = "default_24")]
    pub retention_check_interval_hours: u32,
    #[serde(default = "default_500")]
    pub max_db_size_mb: u32,
    #[serde(default = "default_7")]
    pub aggregate_after_days: u32,
}

impl Default for DatabaseConfig {
    fn default() -> Self {
        Self {
            db_path: default_db_path(),
            retention_days: default_retention_days(),
            retention_check_interval_hours: default_24(),
            max_db_size_mb: default_500(),
            aggregate_after_days: default_7(),
        }
    }
}

fn default_db_path() -> String {
    "/var/lib/egpu-manager/events.db".to_string()
}
fn default_retention_days() -> u32 {
    90
}
fn default_24() -> u32 {
    24
}
fn default_500() -> u32 {
    500
}
fn default_7() -> u32 {
    7
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GpuConfig {
    pub egpu_pci_address: String,
    pub internal_pci_address: String,
    #[serde(default = "default_5")]
    pub poll_interval_seconds: u64,
    #[serde(default = "default_1")]
    pub fast_poll_interval_seconds: u64,
    #[serde(default = "default_3")]
    pub aer_warning_threshold: u64,
    #[serde(default = "default_10")]
    pub aer_burst_threshold: u64,
    #[serde(default = "default_60")]
    pub aer_window_seconds: u64,
    #[serde(default = "default_70")]
    pub bandwidth_warning_percent: u32,
    #[serde(default = "default_85")]
    pub bandwidth_hard_limit_percent: u32,
    #[serde(default = "default_90")]
    pub compute_warning_percent: u32,
    #[serde(default = "default_70")]
    pub compute_soft_limit_percent: u32,
    #[serde(default = "default_5")]
    pub nvidia_smi_timeout_seconds: u64,
    #[serde(default = "default_15")]
    pub nvidia_smi_retry_interval_seconds: u64,
    #[serde(default = "default_3_u32")]
    pub nvidia_smi_max_consecutive_timeouts: u32,
    #[serde(default = "default_120")]
    pub warning_cooldown_seconds: u64,
    #[serde(default = "default_512")]
    pub display_vram_reserve_mb: u64,
    #[serde(default = "default_500_u64")]
    pub link_health_check_interval_ms: u64,
    #[serde(default = "default_throttle")]
    pub link_degradation_action: String,
    #[serde(default = "default_true")]
    pub cuda_watchdog_enabled: bool,
    #[serde(default = "default_500_u64")]
    pub cuda_watchdog_interval_ms: u64,
    #[serde(default = "default_2000")]
    pub cuda_watchdog_timeout_ms: u64,
    #[serde(default = "default_watchdog_binary")]
    pub cuda_watchdog_binary: String,

    // --- Health Score ---
    #[serde(default = "default_3_0")]
    pub health_score_aer_penalty: f64,
    #[serde(default = "default_5_0")]
    pub health_score_pcie_error_penalty: f64,
    #[serde(default = "default_2_0")]
    pub health_score_smi_slow_penalty: f64,
    #[serde(default = "default_5_0")]
    pub health_score_thermal_penalty: f64,
    #[serde(default = "default_1_0")]
    pub health_score_recovery_per_minute: f64,
    #[serde(default = "default_60_0")]
    pub health_score_warning_threshold: f64,
    #[serde(default = "default_40_0")]
    pub health_score_critical_threshold: f64,

    // --- Thermal Proaktiv ---
    #[serde(default = "default_85")]
    pub thermal_throttle_temp_c: u32,
    #[serde(default = "default_90")]
    pub thermal_critical_temp_c: u32,
    #[serde(default = "default_5_0")]
    pub thermal_gradient_warning_c_per_min: f64,

    // --- P-State Proaktiv ---
    #[serde(default = "default_4")]
    pub pstate_throttle_threshold: u32,
    #[serde(default = "default_30")]
    pub pstate_throttle_sustained_seconds: u64,

    // --- nvidia-smi Latenz ---
    #[serde(default = "default_2000")]
    pub nvidia_smi_slow_threshold_ms: u64,
    #[serde(default = "default_10")]
    pub nvidia_smi_response_avg_window: u64,

    // --- Adaptive Polling + Druckreduktion ---
    #[serde(default = "default_500_u64")]
    pub emergency_poll_interval_ms: u64,
    #[serde(default = "default_10")]
    pub pressure_reduction_wait_seconds: u64,
}

fn default_1() -> u64 {
    1
}
fn default_3() -> u64 {
    3
}
fn default_5() -> u64 {
    5
}
fn default_10() -> u64 {
    10
}
fn default_15() -> u64 {
    15
}
fn default_60() -> u64 {
    60
}
fn default_70() -> u32 {
    70
}
fn default_85() -> u32 {
    85
}
fn default_90() -> u32 {
    90
}
fn default_120() -> u64 {
    120
}
fn default_500_u64() -> u64 {
    500
}
fn default_512() -> u64 {
    512
}
fn default_2000() -> u64 {
    2000
}
fn default_true() -> bool {
    true
}
fn default_throttle() -> String {
    "throttle".to_string()
}
fn default_watchdog_binary() -> String {
    "/usr/lib/egpu-manager/egpu-watchdog".to_string()
}
fn default_1_0() -> f64 {
    1.0
}
fn default_2_0() -> f64 {
    2.0
}
fn default_3_0() -> f64 {
    3.0
}
fn default_5_0() -> f64 {
    5.0
}
fn default_40_0() -> f64 {
    40.0
}
fn default_60_0() -> f64 {
    60.0
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThunderboltConfig {
    pub device_uuid: String,
    pub device_path: String,
    #[serde(default = "default_iommu")]
    pub authorized_policy: String,
}

fn default_iommu() -> String {
    "iommu".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DockerConfig {
    #[serde(default = "default_docker_socket")]
    pub socket: String,
    #[serde(default = "default_10_u64")]
    pub api_timeout_seconds: u64,
    #[serde(default = "default_3_u32")]
    pub api_max_retries: u32,
    #[serde(default = "default_10_u64")]
    pub container_stop_timeout_seconds: u64,
    #[serde(default = "default_60_u64")]
    pub container_restart_timeout_seconds: u64,
}

impl Default for DockerConfig {
    fn default() -> Self {
        Self {
            socket: default_docker_socket(),
            api_timeout_seconds: default_10_u64(),
            api_max_retries: default_3_u32(),
            container_stop_timeout_seconds: default_10_u64(),
            container_restart_timeout_seconds: default_60_u64(),
        }
    }
}

fn default_docker_socket() -> String {
    "/var/run/docker.sock".to_string()
}
fn default_10_u64() -> u64 {
    10
}
fn default_3_u32() -> u32 {
    3
}
fn default_60_u64() -> u64 {
    60
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[derive(Default)]
pub struct LocalApiConfig {
    #[serde(default)]
    pub cors_origins: Vec<String>,
}

// Default is derived (cors_origins defaults to empty Vec)

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemoteConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_bind")]
    pub bind: String,
    #[serde(default = "default_7843")]
    pub port: u16,
    #[serde(default)]
    pub token_path: String,
    #[serde(default)]
    pub tls: bool,
    #[serde(default)]
    pub tls_cert: String,
    #[serde(default)]
    pub tls_key: String,
    #[serde(default)]
    pub tls_ca: String,
    #[serde(default)]
    pub ollama_version_pin: String,
}

fn default_bind() -> String {
    "0.0.0.0".to_string()
}
fn default_7843() -> u16 {
    7843
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OllamaConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_ollama_host")]
    pub host: String,
    #[serde(default = "default_5")]
    pub poll_interval_seconds: u64,
    pub gpu_device: String,
    pub fallback_device: String,
    #[serde(default = "default_helper_service")]
    pub fallback_method: String,
    #[serde(default)]
    pub helper_service: String,
    #[serde(default)]
    pub gpu_target_file: String,
    #[serde(default = "default_1_u32")]
    pub priority: u32,
    #[serde(default = "default_14000")]
    pub max_vram_mb: u64,
    #[serde(default = "default_10_u64")]
    pub auto_unload_idle_minutes: u64,
    /// GPU temperature (°C) above which idle Ollama models are preemptively unloaded.
    /// Default: 75°C. Set to 0 to disable thermal unloading.
    #[serde(default = "default_75")]
    pub thermal_unload_temp_c: u32,
    #[serde(default)]
    pub model_tiers: Option<ModelTiers>,
}

fn default_ollama_host() -> String {
    "http://localhost:11434".to_string()
}
fn default_helper_service() -> String {
    "helper-service".to_string()
}
fn default_1_u32() -> u32 {
    1
}
fn default_14000() -> u64 {
    14000
}
fn default_75() -> u32 {
    75
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelTiers {
    pub egpu_available: String,
    pub internal_only: String,
    pub cpu_only: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct NotificationsConfig {
    #[serde(default)]
    pub ntfy_url: String,
    #[serde(default)]
    pub ntfy_topic: String,
    #[serde(default)]
    pub log_path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecoveryConfig {
    #[serde(default = "default_4")]
    pub max_attempts: u32,
    #[serde(default = "default_30")]
    pub reset_cooldown_seconds: u64,
    #[serde(default = "default_5")]
    pub scheduling_lock_timeout_seconds: u64,
}

impl Default for RecoveryConfig {
    fn default() -> Self {
        Self {
            max_attempts: default_4(),
            reset_cooldown_seconds: default_30(),
            scheduling_lock_timeout_seconds: default_5(),
        }
    }
}

fn default_4() -> u32 {
    4
}
fn default_30() -> u64 {
    30
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DaemonConfig {
    #[serde(default = "default_15")]
    pub shutdown_timeout_seconds: u64,
    #[serde(default = "default_30")]
    pub degraded_mode_retry_seconds: u64,
}

impl Default for DaemonConfig {
    fn default() -> Self {
        Self {
            shutdown_timeout_seconds: default_15(),
            degraded_mode_retry_seconds: default_30(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuiesceHook {
    pub container: String,
    pub command: String,
    #[serde(default = "default_5")]
    pub timeout_seconds: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PipelineConfig {
    pub project: String,
    pub container: String,
    pub compose_file: String,
    pub compose_service: String,
    #[serde(default)]
    pub workload_types: Vec<String>,
    #[serde(default = "default_3_u32")]
    pub gpu_priority: u32,
    pub gpu_device: String,
    pub cuda_fallback_device: String,
    #[serde(default)]
    pub vram_estimate_mb: u64,
    #[serde(default)]
    pub exclusive_gpu: bool,
    #[serde(default)]
    pub restart_on_fallback: bool,
    #[serde(default)]
    pub redis_containers: Vec<String>,
    #[serde(default)]
    pub depends_on: Vec<String>,
    #[serde(default)]
    pub remote_capable: Vec<String>,
    #[serde(default)]
    pub cuda_only: Vec<String>,
    #[serde(default)]
    pub quiesce_hooks: Vec<QuiesceHook>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemoteGpuConfig {
    pub name: String,
    pub host: String,
    #[serde(default = "default_11434")]
    pub port_ollama: u16,
    #[serde(default = "default_8080")]
    pub port_llama_cpp: u16,
    #[serde(default = "default_7843")]
    pub port_egpu_agent: u16,
    #[serde(default)]
    pub gpu_name: String,
    #[serde(default)]
    pub vram_mb: u64,
    #[serde(default = "default_on_demand")]
    pub availability: String,
    #[serde(default = "default_30")]
    pub check_interval_seconds: u64,
    #[serde(default = "default_5")]
    pub connection_timeout_seconds: u64,
    #[serde(default = "default_3_u32")]
    pub priority: u32,
    #[serde(default)]
    pub auto_assign: bool,
    #[serde(default)]
    pub max_latency_ms: HashMap<String, u64>,
}

fn default_11434() -> u16 {
    11434
}
fn default_8080() -> u16 {
    8080
}
fn default_on_demand() -> String {
    "on-demand".to_string()
}

fn default_ollama_provider() -> String {
    "ollama".to_string()
}

fn default_openai_compatible() -> String {
    "openai_compatible".to_string()
}

/// LLM Gateway Konfiguration
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct LlmGatewayConfig {
    #[serde(default)]
    pub enabled: bool,
    /// Default provider for apps that don't specify one
    #[serde(default = "default_ollama_provider")]
    pub default_provider: String,
    /// Global rate limit (requests per minute, 0 = unlimited)
    #[serde(default)]
    pub global_rate_limit_rpm: u32,
    /// Monthly budget limit in USD (0 = unlimited)
    #[serde(default)]
    pub monthly_budget_usd: f64,
    #[serde(default)]
    pub providers: Vec<LlmProviderConfig>,
    #[serde(default)]
    pub app_routing: Vec<AppRoutingConfig>,
}

/// Provider-Konfiguration fuer einen LLM-Anbieter
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmProviderConfig {
    pub name: String,
    /// Provider type: "openai_compatible", "anthropic", "gemini"
    #[serde(default = "default_openai_compatible")]
    pub provider_type: String,
    /// API base URL
    pub base_url: String,
    /// API key reference (name in llm-secrets.toml)
    #[serde(default)]
    pub api_key_ref: String,
    /// Available models
    #[serde(default)]
    pub models: Vec<String>,
    /// Rate limit for this provider (requests per minute)
    #[serde(default)]
    pub rate_limit_rpm: u32,
    /// Whether this provider is currently enabled
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Priority (lower = preferred). Used for failover.
    #[serde(default = "default_1_u32")]
    pub priority: u32,
    /// Cost per 1M input tokens in USD (for budget tracking)
    #[serde(default)]
    pub cost_per_1m_input_tokens: f64,
    /// Cost per 1M output tokens in USD
    #[serde(default)]
    pub cost_per_1m_output_tokens: f64,
}

/// App-spezifische Routing-Konfiguration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppRoutingConfig {
    /// App identifier (e.g. "audit_designer", "flowinvoice")
    pub app_id: String,
    /// Allowed providers for this app
    #[serde(default)]
    pub allowed_providers: Vec<String>,
    /// Preferred provider
    #[serde(default)]
    pub preferred_provider: String,
    /// App-specific rate limit (requests per minute, 0 = use global)
    #[serde(default)]
    pub rate_limit_rpm: u32,
    /// App-specific monthly budget in USD (0 = use global)
    #[serde(default)]
    pub monthly_budget_usd: f64,
    /// Allowed models (empty = all from allowed providers)
    #[serde(default)]
    pub allowed_models: Vec<String>,
}

impl Config {
    pub fn load(path: &Path) -> anyhow::Result<Self> {
        let content = std::fs::read_to_string(path)
            .map_err(|e| anyhow::anyhow!("Konfiguration nicht lesbar: {path:?}: {e}"))?;
        let config: Config = toml::from_str(&content)
            .map_err(|e| anyhow::anyhow!("Konfiguration ungültig: {e}"))?;
        config.validate()?;
        Ok(config)
    }

    pub fn validate(&self) -> anyhow::Result<()> {
        if self.schema_version != 1 {
            anyhow::bail!(
                "Schema-Version {} nicht unterstützt (erwartet: 1)",
                self.schema_version
            );
        }

        // PCI-Adressen-Format validieren
        validate_pci_address(&self.gpu.egpu_pci_address)?;
        validate_pci_address(&self.gpu.internal_pci_address)?;

        if self.gpu.egpu_pci_address == self.gpu.internal_pci_address {
            anyhow::bail!("eGPU und interne GPU haben die gleiche PCI-Adresse");
        }

        for (i, p) in self.pipeline.iter().enumerate() {
            validate_pci_address(&p.gpu_device)
                .map_err(|e| anyhow::anyhow!("Pipeline #{i} ({}): {e}", p.container))?;
            validate_pci_address(&p.cuda_fallback_device)
                .map_err(|e| anyhow::anyhow!("Pipeline #{i} ({}): Fallback: {e}", p.container))?;

            if p.gpu_priority == 0 || p.gpu_priority > 5 {
                anyhow::bail!(
                    "Pipeline {} hat ungültige Priorität {} (erlaubt: 1–5)",
                    p.container,
                    p.gpu_priority
                );
            }

            if !std::path::Path::new(&p.compose_file).exists() {
                tracing::warn!(
                    "Pipeline {}: compose_file existiert nicht: {}",
                    p.container,
                    p.compose_file
                );
            }
        }

        Ok(())
    }
}

fn validate_pci_address(addr: &str) -> anyhow::Result<()> {
    // Format: DDDD:BB:DD.F oder 0000DDDD:BB:DD.F
    let parts: Vec<&str> = addr.split(':').collect();
    if parts.len() < 3 {
        anyhow::bail!("Ungültige PCI-Adresse: {addr} (erwartet Format DDDD:BB:DD.F)");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_minimal_config() {
        let toml_str = r#"
            schema_version = 1

            [gpu]
            egpu_pci_address = "0000:05:00.0"
            internal_pci_address = "0000:02:00.0"
        "#;

        let config: Config = toml::from_str(toml_str).unwrap();
        assert_eq!(config.schema_version, 1);
        assert_eq!(config.gpu.egpu_pci_address, "0000:05:00.0");
        assert_eq!(config.gpu.poll_interval_seconds, 5);
        assert!(config.pipeline.is_empty());
        config.validate().unwrap();
    }

    #[test]
    fn test_full_config_with_pipeline() {
        let toml_str = r#"
            schema_version = 1

            [gpu]
            egpu_pci_address = "0000:05:00.0"
            internal_pci_address = "0000:02:00.0"

            [[pipeline]]
            project = "audit_designer"
            container = "audit_designer_celery_worker"
            compose_file = "/tmp/test-compose.yml"
            compose_service = "celery-worker"
            workload_types = ["ocr", "embeddings"]
            gpu_priority = 1
            gpu_device = "0000:05:00.0"
            cuda_fallback_device = "0000:02:00.0"
            vram_estimate_mb = 7168
        "#;

        let config: Config = toml::from_str(toml_str).unwrap();
        assert_eq!(config.pipeline.len(), 1);
        assert_eq!(config.pipeline[0].gpu_priority, 1);
        assert_eq!(config.pipeline[0].vram_estimate_mb, 7168);
    }

    #[test]
    fn test_invalid_schema_version() {
        let toml_str = r#"
            schema_version = 99

            [gpu]
            egpu_pci_address = "0000:05:00.0"
            internal_pci_address = "0000:02:00.0"
        "#;

        let config: Config = toml::from_str(toml_str).unwrap();
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_invalid_priority() {
        let toml_str = r#"
            schema_version = 1

            [gpu]
            egpu_pci_address = "0000:05:00.0"
            internal_pci_address = "0000:02:00.0"

            [[pipeline]]
            project = "test"
            container = "test_worker"
            compose_file = "/tmp/test.yml"
            compose_service = "worker"
            gpu_priority = 0
            gpu_device = "0000:05:00.0"
            cuda_fallback_device = "0000:02:00.0"
        "#;

        let config: Config = toml::from_str(toml_str).unwrap();
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_same_pci_address_rejected() {
        let toml_str = r#"
            schema_version = 1

            [gpu]
            egpu_pci_address = "0000:05:00.0"
            internal_pci_address = "0000:05:00.0"
        "#;

        let config: Config = toml::from_str(toml_str).unwrap();
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_ollama_config() {
        let toml_str = r#"
            schema_version = 1

            [gpu]
            egpu_pci_address = "0000:05:00.0"
            internal_pci_address = "0000:02:00.0"

            [ollama]
            enabled = true
            gpu_device = "0000:05:00.0"
            fallback_device = "0000:02:00.0"
            priority = 1
            max_vram_mb = 14000

            [ollama.model_tiers]
            egpu_available = "qwen3:14b"
            internal_only = "qwen3:8b"
            cpu_only = "qwen3:1.7b"
        "#;

        let config: Config = toml::from_str(toml_str).unwrap();
        let ollama = config.ollama.unwrap();
        assert!(ollama.enabled);
        let tiers = ollama.model_tiers.unwrap();
        assert_eq!(tiers.egpu_available, "qwen3:14b");
        assert_eq!(tiers.internal_only, "qwen3:8b");
    }
}
