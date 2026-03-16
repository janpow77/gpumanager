use std::collections::HashMap;
use std::pin::Pin;
use std::time::Duration;

use async_trait::async_trait;
use tokio_stream::Stream;

use crate::error::*;
use crate::gpu::*;

/// GPU-Monitoring über nvidia-smi
#[async_trait]
pub trait GpuMonitor: Send + Sync {
    async fn query_gpu_status(&self) -> Result<Vec<GpuStatus>, GpuError>;
    async fn query_pcie_throughput(&self, pci_address: &str) -> Result<PcieThroughput, GpuError>;
    async fn query_process_vram(&self, pci_address: &str) -> Result<Vec<ProcessVram>, GpuError>;
}

/// AER-Fehlerzähler aus sysfs
#[async_trait]
pub trait AerMonitor: Send + Sync {
    async fn read_nonfatal_count(&self, pci_address: &str) -> Result<u64, AerError>;
    /// Liest korrigierbare AER-Fehler (aer_dev_correctable).
    /// Default-Implementierung gibt 0 zurueck (abwaertskompatibel).
    async fn read_correctable_count(&self, pci_address: &str) -> Result<u64, AerError> {
        let _ = pci_address;
        Ok(0)
    }
}

/// PCIe-Link-Health aus sysfs
#[async_trait]
pub trait PcieLinkMonitor: Send + Sync {
    async fn read_link_health(&self, pci_address: &str) -> Result<PcieLinkHealth, PcieError>;
}

/// Kernel-Log-Monitoring (/dev/kmsg)
#[derive(Debug, Clone)]
pub struct KmsgEntry {
    pub timestamp: chrono::DateTime<chrono::Utc>,
    pub message: String,
}

#[async_trait]
pub trait KmsgMonitor: Send + Sync {
    async fn subscribe(
        &self,
    ) -> Result<Pin<Box<dyn Stream<Item = KmsgEntry> + Send>>, std::io::Error>;
}

/// CUDA-Watchdog
#[async_trait]
pub trait CudaWatchdog: Send + Sync {
    async fn start(&self) -> Result<(), WatchdogError>;
    async fn is_alive(&self) -> Result<bool, WatchdogError>;
    async fn stop(&self) -> Result<(), WatchdogError>;
}

/// PCIe-Reset-Steuerung
#[async_trait]
pub trait PcieControl: Send + Sync {
    async fn function_level_reset(&self, pci_address: &str) -> Result<(), PcieError>;
}

/// Thunderbolt-Steuerung
#[async_trait]
pub trait ThunderboltControl: Send + Sync {
    async fn deauthorize(&self, device_path: &str) -> Result<(), ThunderboltError>;
    async fn authorize(&self, device_path: &str) -> Result<(), ThunderboltError>;
    async fn is_authorized(&self, device_path: &str) -> Result<bool, ThunderboltError>;
}

/// Container-Info
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContainerInfo {
    pub name: String,
    pub status: String,
    pub running: bool,
}

use serde::{Deserialize, Serialize};

/// Docker-Steuerung
#[async_trait]
pub trait DockerControl: Send + Sync {
    async fn recreate_with_env(
        &self,
        compose_file: &str,
        service: &str,
        env: HashMap<String, String>,
    ) -> Result<(), DockerError>;

    async fn exec_in_container(
        &self,
        name: &str,
        cmd: &[&str],
        timeout: Duration,
    ) -> Result<String, DockerError>;

    async fn stop_container(&self, name: &str, timeout: Duration) -> Result<(), DockerError>;

    async fn list_containers(&self) -> Result<Vec<ContainerInfo>, DockerError>;
}

/// Ollama-Steuerung
#[async_trait]
pub trait OllamaControl: Send + Sync {
    async fn list_running_models(&self) -> Result<Vec<OllamaModel>, OllamaError>;
    async fn unload_model(&self, model: &str) -> Result<(), OllamaError>;
    async fn get_vram_usage(&self) -> Result<u64, OllamaError>;
}
