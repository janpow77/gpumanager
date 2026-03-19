use std::time::Duration;

use async_trait::async_trait;
use egpu_manager_common::error::OllamaError;
use egpu_manager_common::gpu::OllamaModel;
use egpu_manager_common::hal::OllamaControl;
use tracing::{debug, info, warn};

/// Real Ollama control via HTTP API.
pub struct HttpOllamaControl {
    host: String,
    client: reqwest::Client,
}

impl HttpOllamaControl {
    pub fn new(host: &str) -> Self {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(10))
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());

        Self {
            host: host.to_string(),
            client,
        }
    }
}

#[async_trait]
impl OllamaControl for HttpOllamaControl {
    /// List running models via GET /api/ps.
    async fn list_running_models(&self) -> Result<Vec<OllamaModel>, OllamaError> {
        let url = format!("{}/api/ps", self.host);
        let resp = self
            .client
            .get(&url)
            .send()
            .await
            .map_err(|e| OllamaError::Unreachable(format!("{url}: {e}")))?;

        if !resp.status().is_success() {
            return Err(OllamaError::ApiError(format!(
                "HTTP {}",
                resp.status()
            )));
        }

        let body: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| OllamaError::ApiError(format!("JSON-Parse: {e}")))?;

        let models = body["models"]
            .as_array()
            .unwrap_or(&vec![])
            .iter()
            .filter_map(|m| {
                Some(OllamaModel {
                    name: m["name"].as_str()?.to_string(),
                    size_bytes: m["size"].as_u64().unwrap_or(0),
                    size_vram_bytes: m["size_vram"].as_u64().unwrap_or(0),
                    expires_at: None,
                })
            })
            .collect();

        Ok(models)
    }

    /// Unload a model via POST /api/generate with keep_alive=0.
    async fn unload_model(&self, model: &str) -> Result<(), OllamaError> {
        let url = format!("{}/api/generate", self.host);
        let body = serde_json::json!({
            "model": model,
            "keep_alive": 0
        });

        info!("Ollama: Modell {} wird entladen", model);

        let resp = self
            .client
            .post(&url)
            .json(&body)
            .send()
            .await
            .map_err(|e| OllamaError::Unreachable(format!("{url}: {e}")))?;

        if !resp.status().is_success() {
            return Err(OllamaError::ApiError(format!(
                "Unload fehlgeschlagen: HTTP {}",
                resp.status()
            )));
        }

        info!("Ollama: Modell {} entladen", model);
        Ok(())
    }

    /// Get total VRAM usage by summing size_vram from running models.
    async fn get_vram_usage(&self) -> Result<u64, OllamaError> {
        let models = self.list_running_models().await?;
        let total: u64 = models.iter().map(|m| m.size_vram_bytes).sum();
        Ok(total)
    }
}


// ─── OllamaFleet: Multi-Instanz-Management ───────────────────────────────

/// Wrapper um eine einzelne Ollama-Instanz mit GPU-Zuordnung.
pub struct OllamaInstanceControl {
    pub config: egpu_manager_common::config::OllamaInstanceConfig,
    pub control: HttpOllamaControl,
    pub gpu_target: crate::scheduler::GpuTarget,
    pub available: bool,
}

/// Manager für mehrere Ollama-Instanzen auf verschiedenen GPUs.
pub struct OllamaFleet {
    instances: Vec<OllamaInstanceControl>,
}

impl OllamaFleet {
    /// Erstellt eine neue Fleet aus Config-Instanzen.
    pub fn new(
        configs: Vec<egpu_manager_common::config::OllamaInstanceConfig>,
        egpu_pci: &str,
    ) -> Self {
        let instances = configs
            .into_iter()
            .map(|cfg| {
                let gpu_target = if cfg.gpu_device == egpu_pci {
                    crate::scheduler::GpuTarget::Egpu
                } else {
                    crate::scheduler::GpuTarget::Internal
                };
                let control = HttpOllamaControl::new(&cfg.host);
                OllamaInstanceControl {
                    config: cfg,
                    control,
                    gpu_target,
                    available: true,
                }
            })
            .collect();
        Self { instances }
    }

    /// Gibt Instanzen zurück die den gegebenen Workload-Typ unterstützen.
    pub fn instances_for_workload(&self, workload_type: &str) -> Vec<&OllamaInstanceControl> {
        self.instances
            .iter()
            .filter(|i| i.available && i.config.workload_types.iter().any(|w| w == workload_type))
            .collect()
    }

    /// Findet eine Instanz anhand ihres Namens.
    pub fn instance_by_name(&self, name: &str) -> Option<&OllamaInstanceControl> {
        self.instances.iter().find(|i| i.config.name == name)
    }

    /// Findet die Instanz die ein bestimmtes Modell konfiguriert hat.
    pub fn instance_for_model(&self, model: &str) -> Option<&OllamaInstanceControl> {
        self.instances
            .iter()
            .filter(|i| i.available)
            .find(|i| i.config.models.iter().any(|m| m == model))
    }

    /// Markiert alle Instanzen auf einer bestimmten GPU als (un-)verfügbar.
    pub fn set_gpu_available(&mut self, target: crate::scheduler::GpuTarget, available: bool) {
        for instance in &mut self.instances {
            if instance.gpu_target == target {
                if instance.available != available {
                    if available {
                        info!(
                            "Ollama-Instanz '{}' wieder verfügbar (GPU {})",
                            instance.config.name, target
                        );
                    } else {
                        warn!(
                            "Ollama-Instanz '{}' nicht verfügbar (GPU {} offline)",
                            instance.config.name, target
                        );
                    }
                    instance.available = available;
                }
            }
        }
    }

    /// Pollt alle verfügbaren Instanzen und gibt Modelle pro Instanz zurück.
    pub async fn query_all_models(&self) -> HashMap<String, Vec<OllamaModel>> {
        let mut result = HashMap::new();
        for instance in &self.instances {
            if !instance.available {
                continue;
            }
            match instance.control.list_running_models().await {
                Ok(models) => {
                    result.insert(instance.config.name.clone(), models);
                }
                Err(e) => {
                    debug!(
                        "Ollama-Instanz '{}' nicht erreichbar: {}",
                        instance.config.name, e
                    );
                }
            }
        }
        result
    }

    /// Entlädt ein Modell auf einer bestimmten Instanz.
    pub async fn unload_model_on(
        &self,
        instance_name: &str,
        model: &str,
    ) -> Result<(), OllamaError> {
        let instance = self
            .instance_by_name(instance_name)
            .ok_or_else(|| OllamaError::ApiError(format!("Instanz '{}' nicht gefunden", instance_name)))?;
        instance.control.unload_model(model).await
    }

    /// Gibt alle konfigurierten Instanzen zurück (auch nicht-verfügbare).
    pub fn all_instances(&self) -> &[OllamaInstanceControl] {
        &self.instances
    }

}

use std::collections::HashMap;

/// Mock Ollama control for testing.
#[cfg(any(test, feature = "mock-hardware"))]
pub struct MockOllamaControl {
    pub models: std::sync::Arc<tokio::sync::Mutex<Vec<OllamaModel>>>,
    pub unloaded: std::sync::Arc<tokio::sync::Mutex<Vec<String>>>,
}

#[cfg(any(test, feature = "mock-hardware"))]
impl MockOllamaControl {
    pub fn new() -> Self {
        Self {
            models: std::sync::Arc::new(tokio::sync::Mutex::new(Vec::new())),
            unloaded: std::sync::Arc::new(tokio::sync::Mutex::new(Vec::new())),
        }
    }

    pub fn with_models(models: Vec<OllamaModel>) -> Self {
        Self {
            models: std::sync::Arc::new(tokio::sync::Mutex::new(models)),
            unloaded: std::sync::Arc::new(tokio::sync::Mutex::new(Vec::new())),
        }
    }
}

#[cfg(any(test, feature = "mock-hardware"))]
#[async_trait]
impl OllamaControl for MockOllamaControl {
    async fn list_running_models(&self) -> Result<Vec<OllamaModel>, OllamaError> {
        let models = self.models.lock().await;
        Ok(models.clone())
    }

    async fn unload_model(&self, model: &str) -> Result<(), OllamaError> {
        let mut unloaded = self.unloaded.lock().await;
        unloaded.push(model.to_string());

        // Also remove from running models
        let mut models = self.models.lock().await;
        models.retain(|m| m.name != model);

        Ok(())
    }

    async fn get_vram_usage(&self) -> Result<u64, OllamaError> {
        let models = self.models.lock().await;
        Ok(models.iter().map(|m| m.size_vram_bytes).sum())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_mock_ollama_list() {
        let ollama = MockOllamaControl::with_models(vec![OllamaModel {
            name: "qwen3:14b".to_string(),
            size_bytes: 14_000_000_000,
            size_vram_bytes: 12_000_000_000,
            expires_at: None,
        }]);

        let models = ollama.list_running_models().await.unwrap();
        assert_eq!(models.len(), 1);
        assert_eq!(models[0].name, "qwen3:14b");
    }

    #[tokio::test]
    async fn test_mock_ollama_unload() {
        let ollama = MockOllamaControl::with_models(vec![OllamaModel {
            name: "qwen3:14b".to_string(),
            size_bytes: 14_000_000_000,
            size_vram_bytes: 12_000_000_000,
            expires_at: None,
        }]);

        ollama.unload_model("qwen3:14b").await.unwrap();

        let unloaded = ollama.unloaded.lock().await;
        assert_eq!(unloaded.len(), 1);
        assert_eq!(unloaded[0], "qwen3:14b");

        let models = ollama.list_running_models().await.unwrap();
        assert!(models.is_empty());
    }

    #[tokio::test]
    async fn test_mock_ollama_vram() {
        let ollama = MockOllamaControl::with_models(vec![
            OllamaModel {
                name: "model_a".to_string(),
                size_bytes: 10_000_000_000,
                size_vram_bytes: 5_000_000_000,
                expires_at: None,
            },
            OllamaModel {
                name: "model_b".to_string(),
                size_bytes: 8_000_000_000,
                size_vram_bytes: 3_000_000_000,
                expires_at: None,
            },
        ]);

        let vram = ollama.get_vram_usage().await.unwrap();
        assert_eq!(vram, 8_000_000_000);
    }

    #[test]
    fn test_fleet_workload_routing() {
        use egpu_manager_common::config::OllamaInstanceConfig;

        let configs = vec![
            OllamaInstanceConfig {
                name: "ollama-egpu".to_string(),
                host: "http://localhost:11434".to_string(),
                gpu_device: "0000:05:00.0".to_string(),
                models: vec!["qwen3:14b".to_string(), "nomic-embed-text".to_string()],
                workload_types: vec!["llm".to_string(), "embeddings".to_string()],
                priority: 1,
                max_vram_mb: 14000,
                auto_unload_idle_minutes: 10,
                thermal_unload_temp_c: 75,
            },
            OllamaInstanceConfig {
                name: "ollama-internal".to_string(),
                host: "http://localhost:11435".to_string(),
                gpu_device: "0000:02:00.0".to_string(),
                models: vec!["qwen3:8b".to_string()],
                workload_types: vec!["ocr-assist".to_string(), "staging".to_string()],
                priority: 2,
                max_vram_mb: 6000,
                auto_unload_idle_minutes: 10,
                thermal_unload_temp_c: 75,
            },
        ];

        let fleet = OllamaFleet::new(configs, "0000:05:00.0");
        assert_eq!(fleet.all_instances().len(), 2);

        // LLM-Workload → ollama-egpu
        let llm_instances = fleet.instances_for_workload("llm");
        assert_eq!(llm_instances.len(), 1);
        assert_eq!(llm_instances[0].config.name, "ollama-egpu");

        // staging → ollama-internal
        let staging_instances = fleet.instances_for_workload("staging");
        assert_eq!(staging_instances.len(), 1);
        assert_eq!(staging_instances[0].config.name, "ollama-internal");

        // unbekannter Workload → keine Instanz
        let unknown = fleet.instances_for_workload("unknown");
        assert!(unknown.is_empty());

        // Modell-Lookup
        let qwen14b = fleet.instance_for_model("qwen3:14b");
        assert!(qwen14b.is_some());
        assert_eq!(qwen14b.unwrap().config.name, "ollama-egpu");

        let qwen8b = fleet.instance_for_model("qwen3:8b");
        assert!(qwen8b.is_some());
        assert_eq!(qwen8b.unwrap().config.name, "ollama-internal");
    }

    #[test]
    fn test_fleet_gpu_unavailable() {
        use egpu_manager_common::config::OllamaInstanceConfig;
        use crate::scheduler::GpuTarget;

        let configs = vec![
            OllamaInstanceConfig {
                name: "ollama-egpu".to_string(),
                host: "http://localhost:11434".to_string(),
                gpu_device: "0000:05:00.0".to_string(),
                models: vec!["qwen3:14b".to_string()],
                workload_types: vec!["llm".to_string()],
                priority: 1,
                max_vram_mb: 14000,
                auto_unload_idle_minutes: 10,
                thermal_unload_temp_c: 75,
            },
            OllamaInstanceConfig {
                name: "ollama-internal".to_string(),
                host: "http://localhost:11435".to_string(),
                gpu_device: "0000:02:00.0".to_string(),
                models: vec!["qwen3:8b".to_string()],
                workload_types: vec!["llm".to_string()],
                priority: 2,
                max_vram_mb: 6000,
                auto_unload_idle_minutes: 10,
                thermal_unload_temp_c: 75,
            },
        ];

        let mut fleet = OllamaFleet::new(configs, "0000:05:00.0");

        // Beide Instanzen verfügbar für LLM
        assert_eq!(fleet.instances_for_workload("llm").len(), 2);

        // eGPU offline
        fleet.set_gpu_available(GpuTarget::Egpu, false);
        let llm = fleet.instances_for_workload("llm");
        assert_eq!(llm.len(), 1);
        assert_eq!(llm[0].config.name, "ollama-internal");

        // Modell auf eGPU nicht mehr findbar
        assert!(fleet.instance_for_model("qwen3:14b").is_none());
        // Modell auf interner GPU noch verfügbar
        assert!(fleet.instance_for_model("qwen3:8b").is_some());

        // eGPU wieder online
        fleet.set_gpu_available(GpuTarget::Egpu, true);
        assert_eq!(fleet.instances_for_workload("llm").len(), 2);
    }

}
