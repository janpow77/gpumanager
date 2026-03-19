// GTK-Widget deserialisiert JSON-Felder die nicht alle im UI angezeigt werden.
// Das ist by-design (forward-compatible mit neuen API-Feldern).
#![allow(dead_code)]

use serde::Deserialize;

/// Connection state to the daemon.
#[derive(Debug, Clone, PartialEq)]
pub enum ConnectionState {
    Connected,
    Connecting,
    Reconnecting(u32),
    Error(String),
}

/// Full widget state, updated by the polling loop.
#[derive(Debug, Clone)]
pub struct WidgetState {
    pub connection: ConnectionState,
    pub daemon: Option<DaemonStatus>,
    pub health_score: Option<HealthScoreInfo>,
    pub gpus: Vec<GpuInfo>,
    pub remote_gpus: Vec<RemoteGpuInfo>,
    pub pipelines: Vec<PipelineInfo>,
}

impl Default for WidgetState {
    fn default() -> Self {
        Self {
            connection: ConnectionState::Connecting,
            daemon: None,
            health_score: None,
            gpus: Vec::new(),
            remote_gpus: Vec::new(),
            pipelines: Vec::new(),
        }
    }
}

impl WidgetState {
    /// Get the current warning level color name.
    pub fn warning_color(&self) -> &'static str {
        match &self.daemon {
            Some(d) => {
                let lvl = d.warning_level.to_lowercase();
                if lvl.contains("rot") || lvl.contains("red") {
                    "red"
                } else if lvl.contains("orange") {
                    "orange"
                } else if lvl.contains("gelb") || lvl.contains("yellow") {
                    "yellow"
                } else if lvl.contains("gr") {
                    "green"
                } else {
                    "gray"
                }
            }
            None => "gray",
        }
    }
}

// ─── API response types ──────────────────────────────────────────

#[derive(Debug, Clone, Deserialize)]
pub struct StatusResponse {
    pub daemon: Option<DaemonStatus>,
    pub gpus: Option<Vec<GpuInfo>>,
    pub remote_gpus: Option<Vec<RemoteGpuInfo>>,
    pub health_score: Option<HealthScoreInfo>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct HealthScoreInfo {
    #[serde(default = "default_100")]
    pub score: f64,
    #[serde(default)]
    pub warned_low: bool,
    #[serde(default)]
    pub warned_critical: bool,
    #[serde(default)]
    pub event_count: usize,
}

fn default_100() -> f64 {
    100.0
}

#[derive(Debug, Clone, Deserialize)]
pub struct DaemonStatus {
    #[serde(default)]
    pub warning_level: String,
    #[serde(default)]
    pub recovery_active: bool,
    #[serde(default)]
    pub recovery_stage: Option<String>,
    #[serde(default)]
    pub egpu_admission_state: String,
    #[serde(default)]
    pub scheduler_queue_length: u32,
    #[serde(default)]
    pub mode: Option<String>,
    #[serde(default)]
    pub uptime_seconds: Option<u64>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct GpuInfo {
    pub name: String,
    #[serde(rename = "type", default)]
    pub gpu_type: String,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub temperature_c: i32,
    #[serde(default)]
    pub utilization_gpu_percent: u32,
    #[serde(default)]
    pub memory_used_mb: u64,
    #[serde(default)]
    pub memory_total_mb: u64,
    #[serde(default)]
    pub power_draw_w: f64,
    #[serde(default)]
    pub pci_address: String,
    #[serde(default)]
    pub pstate: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RemoteGpuInfo {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub host: String,
    #[serde(default)]
    pub gpu_name: String,
    #[serde(default)]
    pub vram_mb: u64,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub latency_ms: Option<u32>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PipelineInfo {
    #[serde(default)]
    pub container: String,
    #[serde(default)]
    pub project: String,
    #[serde(default)]
    pub priority: u32,
    #[serde(default)]
    pub gpu_type: String,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub vram_estimate_mb: u64,
    #[serde(default)]
    pub actual_vram_mb: Option<u64>,
    #[serde(default)]
    pub workload_types: Vec<String>,
    #[serde(default)]
    pub decision_reason: Option<String>,
    #[serde(default)]
    pub assignment_source: Option<String>,
}
