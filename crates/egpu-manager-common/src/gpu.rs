use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// GPU-Typ
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GpuType {
    Internal,
    Egpu,
    Remote,
}

/// GPU-Status von nvidia-smi
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GpuStatus {
    pub pci_address: String,
    pub nvidia_index: Option<u32>,
    pub name: String,
    pub gpu_type: GpuType,
    pub temperature_c: u32,
    pub utilization_gpu_percent: u32,
    pub memory_used_mb: u64,
    pub memory_free_mb: u64,
    pub memory_total_mb: u64,
    pub power_draw_w: f64,
    pub pstate: String,
    pub fan_speed_percent: u32,
    pub clock_graphics_mhz: u32,
    pub clock_memory_mhz: u32,
    pub throttle_reason: String,
    pub status: GpuOnlineStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GpuOnlineStatus {
    Online,
    Offline,
    Timeout,
    Unknown,
}

/// PCIe-Durchsatz von nvidia-smi dmon
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PcieThroughput {
    pub pci_address: String,
    pub tx_kbps: u64,
    pub rx_kbps: u64,
}

/// PCIe-Link-Zustand aus sysfs
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PcieLinkHealth {
    pub pci_address: String,
    pub current_link_speed: String,
    pub current_link_width: u8,
    pub max_link_speed: String,
    pub max_link_width: u8,
    pub degraded: bool,
}

impl PcieLinkHealth {
    pub fn is_degraded(&self) -> bool {
        self.current_link_width < self.max_link_width
    }

    pub fn is_speed_degraded(&self) -> bool {
        self.current_link_speed != self.max_link_speed
    }

    pub fn is_link_down(&self) -> bool {
        self.current_link_speed == "Unknown" || self.current_link_width == 0
    }
}

/// Warnstufen
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WarningLevel {
    Green,
    Yellow,
    Orange,
    Red,
}

impl std::fmt::Display for WarningLevel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            WarningLevel::Green => write!(f, "Grün"),
            WarningLevel::Yellow => write!(f, "Gelb"),
            WarningLevel::Orange => write!(f, "Orange"),
            WarningLevel::Red => write!(f, "Rot"),
        }
    }
}

/// Ollama-Modell-Info
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OllamaModel {
    pub name: String,
    pub size_bytes: u64,
    pub size_vram_bytes: u64,
    #[serde(default)]
    pub expires_at: Option<DateTime<Utc>>,
}

/// Prozess-VRAM-Verbrauch
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProcessVram {
    pub pid: u32,
    pub used_mb: u64,
    pub process_name: Option<String>,
}

/// CUDA-Watchdog-Status
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WatchdogStatus {
    Ok,
    Timeout,
    NotRunning,
    Disabled,
}

/// Workload update from a pipeline (webhook payload).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkloadUpdate {
    /// Pipeline/container name
    pub pipeline: String,
    /// Current workload type (e.g. "ocr", "embeddings", "inference")
    pub workload_type: String,
    /// Current VRAM usage in MB (measured, not estimated)
    pub vram_mb: u64,
    /// Whether the workload is actively using the GPU
    pub gpu_active: bool,
}

/// Response to a workload update
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkloadUpdateResponse {
    /// Whether the update was accepted
    pub accepted: bool,
    /// Optional message
    #[serde(default)]
    pub message: String,
}
