use std::time::Duration;

use async_trait::async_trait;
use egpu_manager_common::config::OllamaConfig;
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

/// Manage Ollama GPU fallback.
pub struct OllamaManager {
    config: OllamaConfig,
}

impl OllamaManager {
    pub fn new(config: OllamaConfig) -> Self {
        Self { config }
    }

    /// Switch Ollama to fallback GPU via helper service.
    /// Writes target file and triggers systemd service.
    pub async fn switch_to_fallback(&self) -> Result<(), OllamaError> {
        let target_file = if self.config.gpu_target_file.is_empty() {
            "/run/egpu-manager/ollama-gpu-target".to_string()
        } else {
            self.config.gpu_target_file.clone()
        };

        info!(
            "Ollama GPU-Fallback: schreibe {} -> {}",
            target_file, self.config.fallback_device
        );

        // Ensure directory exists
        if let Some(parent) = std::path::Path::new(&target_file).parent()
            && let Err(e) = tokio::fs::create_dir_all(parent).await
        {
            warn!("Verzeichnis nicht erstellbar: {}: {}", parent.display(), e);
        }

        // Write target file
        tokio::fs::write(&target_file, &self.config.fallback_device)
            .await
            .map_err(|e| {
                OllamaError::ApiError(format!(
                    "GPU-Target-Datei nicht schreibbar: {target_file}: {e}"
                ))
            })?;

        // Trigger helper service if configured
        let service_name = if self.config.helper_service.is_empty() {
            "egpu-ollama-fallback.service"
        } else {
            &self.config.helper_service
        };

        info!("Starte Helper-Service: {}", service_name);
        let result = tokio::process::Command::new("sudo")
            .args(["systemctl", "start", service_name])
            .output()
            .await;

        match result {
            Ok(output) if output.status.success() => {
                info!("Helper-Service {} gestartet", service_name);
                Ok(())
            }
            Ok(output) => {
                let stderr = String::from_utf8_lossy(&output.stderr);
                Err(OllamaError::ApiError(format!(
                    "Helper-Service start fehlgeschlagen: {stderr}"
                )))
            }
            Err(e) => Err(OllamaError::ApiError(format!(
                "systemctl nicht ausführbar: {e}"
            ))),
        }
    }

    /// Switch Ollama back to eGPU.
    pub async fn switch_to_egpu(&self) -> Result<(), OllamaError> {
        let target_file = if self.config.gpu_target_file.is_empty() {
            "/run/egpu-manager/ollama-gpu-target".to_string()
        } else {
            self.config.gpu_target_file.clone()
        };

        info!(
            "Ollama GPU-Restore: schreibe {} -> {}",
            target_file, self.config.gpu_device
        );

        tokio::fs::write(&target_file, &self.config.gpu_device)
            .await
            .map_err(|e| {
                OllamaError::ApiError(format!(
                    "GPU-Target-Datei nicht schreibbar: {target_file}: {e}"
                ))
            })?;

        let service_name = if self.config.helper_service.is_empty() {
            "egpu-ollama-fallback.service"
        } else {
            &self.config.helper_service
        };

        let result = tokio::process::Command::new("sudo")
            .args(["systemctl", "start", service_name])
            .output()
            .await;

        match result {
            Ok(output) if output.status.success() => {
                info!("Helper-Service {} gestartet (GPU-Restore)", service_name);
                Ok(())
            }
            Ok(output) => {
                let stderr = String::from_utf8_lossy(&output.stderr);
                Err(OllamaError::ApiError(format!(
                    "Helper-Service start fehlgeschlagen: {stderr}"
                )))
            }
            Err(e) => Err(OllamaError::ApiError(format!(
                "systemctl nicht ausführbar: {e}"
            ))),
        }
    }

    /// Perform model tier switching: unload current models and load
    /// the tier-appropriate model.
    pub async fn switch_model_tier(
        &self,
        ollama: &dyn OllamaControl,
        egpu_available: bool,
    ) -> Result<(), OllamaError> {
        let tiers = match &self.config.model_tiers {
            Some(t) => t,
            None => {
                debug!("Keine Model-Tiers konfiguriert, überspringe Tier-Switch");
                return Ok(());
            }
        };

        // Unload all currently running models
        let running = ollama.list_running_models().await?;
        for model in &running {
            info!("Entlade Modell: {}", model.name);
            ollama.unload_model(&model.name).await?;
        }

        // Determine target model based on GPU availability
        let target_model = if egpu_available {
            &tiers.egpu_available
        } else {
            &tiers.internal_only
        };

        info!(
            "Model-Tier-Switch: {} (eGPU verfügbar: {})",
            target_model, egpu_available
        );

        // Load is triggered implicitly by the next request to Ollama
        // We just log the intent here; actual pre-loading would use /api/generate
        // with a minimal prompt, but that's done by the applications themselves.

        Ok(())
    }
}

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
    use egpu_manager_common::config::ModelTiers;

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

    #[tokio::test]
    async fn test_model_tier_switch() {
        let config = OllamaConfig {
            enabled: true,
            host: "http://localhost:11434".to_string(),
            poll_interval_seconds: 5,
            gpu_device: "0000:05:00.0".to_string(),
            fallback_device: "0000:02:00.0".to_string(),
            fallback_method: "helper-service".to_string(),
            helper_service: String::new(),
            gpu_target_file: String::new(),
            priority: 1,
            max_vram_mb: 14000,
            auto_unload_idle_minutes: 10,
            thermal_unload_temp_c: 75,
            model_tiers: Some(ModelTiers {
                egpu_available: "qwen3:14b".to_string(),
                internal_only: "qwen3:8b".to_string(),
                cpu_only: "qwen3:1.7b".to_string(),
            }),
        };

        let ollama = MockOllamaControl::with_models(vec![OllamaModel {
            name: "qwen3:14b".to_string(),
            size_bytes: 14_000_000_000,
            size_vram_bytes: 12_000_000_000,
            expires_at: None,
        }]);

        let manager = OllamaManager::new(config);

        // Switch to internal-only tier (eGPU unavailable)
        manager
            .switch_model_tier(&ollama, false)
            .await
            .unwrap();

        // The old model should have been unloaded
        let unloaded = ollama.unloaded.lock().await;
        assert_eq!(unloaded.len(), 1);
        assert_eq!(unloaded[0], "qwen3:14b");
    }

    #[tokio::test]
    async fn test_model_tier_switch_no_tiers() {
        let config = OllamaConfig {
            enabled: true,
            host: "http://localhost:11434".to_string(),
            poll_interval_seconds: 5,
            gpu_device: "0000:05:00.0".to_string(),
            fallback_device: "0000:02:00.0".to_string(),
            fallback_method: "helper-service".to_string(),
            helper_service: String::new(),
            gpu_target_file: String::new(),
            priority: 1,
            max_vram_mb: 14000,
            auto_unload_idle_minutes: 10,
            thermal_unload_temp_c: 75,
            model_tiers: None,
        };

        let ollama = MockOllamaControl::new();
        let manager = OllamaManager::new(config);

        // Should succeed without doing anything
        manager
            .switch_model_tier(&ollama, false)
            .await
            .unwrap();

        let unloaded = ollama.unloaded.lock().await;
        assert!(unloaded.is_empty());
    }
}
