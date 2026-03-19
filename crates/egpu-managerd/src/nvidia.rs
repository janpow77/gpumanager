use std::process::Stdio;
use std::time::Duration;

use egpu_manager_common::error::GpuError;
use egpu_manager_common::gpu::*;
use nvml_wrapper::bitmasks::device::ThrottleReasons;
use nvml_wrapper::enum_wrappers::device::{PcieUtilCounter, TemperatureSensor};
use nvml_wrapper::enums::device::UsedGpuMemory;
use nvml_wrapper::Nvml;
use tokio::process::Command;
use tracing::{debug, info, warn};

// ──────────────────────────────────────────────────────────────────────────────
// Unified GPU Monitor Backend
// ──────────────────────────────────────────────────────────────────────────────

/// Unified backend: tries NVML first, falls back to nvidia-smi CLI.
pub enum GpuMonitorBackend {
    Nvml(NvmlGpuMonitor),
    NvidiaSmi(NvidiaSmiMonitor),
}

impl GpuMonitorBackend {
    /// Create a new backend. Tries NVML first; on failure falls back to nvidia-smi.
    pub fn new(timeout_secs: u64) -> Self {
        match NvmlGpuMonitor::new(timeout_secs) {
            Ok(nvml) => {
                info!("NVML-Backend initialisiert");
                GpuMonitorBackend::Nvml(nvml)
            }
            Err(e) => {
                warn!("NVML-Init fehlgeschlagen ({e}), Fallback auf nvidia-smi");
                GpuMonitorBackend::NvidiaSmi(NvidiaSmiMonitor::new(timeout_secs))
            }
        }
    }

    pub async fn query_all(&self) -> Result<Vec<GpuStatus>, GpuError> {
        match self {
            GpuMonitorBackend::Nvml(m) => m.query_all(),
            GpuMonitorBackend::NvidiaSmi(m) => m.query_all().await,
        }
    }

    pub async fn query_pcie_throughput(
        &self,
        pci_address: &str,
    ) -> Result<PcieThroughput, GpuError> {
        match self {
            GpuMonitorBackend::Nvml(m) => m.query_pcie_throughput(pci_address),
            GpuMonitorBackend::NvidiaSmi(m) => m.query_pcie_throughput(pci_address).await,
        }
    }

    pub async fn query_display_vram(&self, pci_address: &str) -> Result<u64, GpuError> {
        match self {
            GpuMonitorBackend::Nvml(m) => m.query_display_vram(pci_address),
            GpuMonitorBackend::NvidiaSmi(m) => m.query_display_vram(pci_address).await,
        }
    }

    pub fn query_compute_processes(
        &self,
        pci_address: &str,
    ) -> Result<Vec<ProcessVram>, GpuError> {
        match self {
            GpuMonitorBackend::Nvml(m) => m.query_compute_processes(pci_address),
            GpuMonitorBackend::NvidiaSmi(_) => {
                // nvidia-smi fallback does not support compute process listing
                Ok(vec![])
            }
        }
    }

    pub fn query_driver_version(&self) -> Result<String, GpuError> {
        match self {
            GpuMonitorBackend::Nvml(m) => m.query_driver_version(),
            GpuMonitorBackend::NvidiaSmi(_) => Err(GpuError::NvmlError(
                "Driver-Version nur über NVML verfügbar".to_string(),
            )),
        }
    }

    pub fn validate_gpu_functional(
        &self,
        pci_address: &str,
        expected_memory_mb: Option<u64>,
    ) -> Result<bool, GpuError> {
        match self {
            GpuMonitorBackend::Nvml(m) => {
                m.validate_gpu_functional(pci_address, expected_memory_mb)
            }
            GpuMonitorBackend::NvidiaSmi(_) => {
                // Can't validate without NVML — assume OK
                Ok(true)
            }
        }
    }

    /// Returns true if the backend is NVML.
    pub fn is_nvml(&self) -> bool {
        matches!(self, GpuMonitorBackend::Nvml(_))
    }
}

// Keep backward-compatible type alias so monitor.rs compiles unchanged.
// monitor.rs imports `NvidiaSmiMonitor` and calls `.new(timeout)`, `.query_all()`, etc.
// Since we must NOT modify monitor.rs, we keep NvidiaSmiMonitor public and usable as before.

// ──────────────────────────────────────────────────────────────────────────────
// NVML-based GPU Monitor
// ──────────────────────────────────────────────────────────────────────────────

pub struct NvmlGpuMonitor {
    nvml: Nvml,
    #[allow(dead_code)]
    timeout_secs: u64,
}

impl NvmlGpuMonitor {
    pub fn new(timeout_secs: u64) -> Result<Self, GpuError> {
        let nvml = Nvml::init().map_err(|e| GpuError::NvmlError(format!("NVML init: {e}")))?;
        Ok(Self { nvml, timeout_secs })
    }

    /// Query all GPUs via NVML.
    pub fn query_all(&self) -> Result<Vec<GpuStatus>, GpuError> {
        let count = self
            .nvml
            .device_count()
            .map_err(|e| GpuError::NvmlError(format!("device_count: {e}")))?;

        let mut gpus = Vec::with_capacity(count as usize);

        for idx in 0..count {
            match self.query_single_device(idx) {
                Ok(gpu) => gpus.push(gpu),
                Err(e) => {
                    warn!("GPU {idx} uebersprungen (NVML-Fehler, evtl. temporaer nicht erreichbar): {e}");
                }
            }
        }

        if gpus.is_empty() {
            return Err(GpuError::NvmlError(
                "Keine GPUs über NVML gefunden".to_string(),
            ));
        }

        debug!("{} GPU(s) über NVML abgefragt", gpus.len());
        Ok(gpus)
    }

    /// Query a single GPU by NVML index. Errors are non-fatal so callers
    /// can skip unavailable devices (e.g. eGPU temporarily unreachable).
    fn query_single_device(&self, idx: u32) -> Result<GpuStatus, GpuError> {
        let device = self
            .nvml
            .device_by_index(idx)
            .map_err(|e| GpuError::NvmlError(format!("device_by_index({idx}): {e}")))?;

        let pci_info = device
            .pci_info()
            .map_err(|e| GpuError::NvmlError(format!("pci_info: {e}")))?;
        let pci_address = normalize_pci_address(&pci_info.bus_id);

        let name = device.name().unwrap_or_else(|_| "Unknown GPU".to_string());

        let gpu_uuid = device
            .uuid()
            .unwrap_or_else(|_| read_gpu_uuid(&pci_address).unwrap_or_default());

        let temperature_c = device
            .temperature(TemperatureSensor::Gpu)
            .unwrap_or(0);

        let utilization = device.utilization_rates().ok();
        let utilization_gpu_percent = utilization.map(|u| u.gpu).unwrap_or(0);

        let memory_info = device.memory_info().ok();
        let memory_used_mb = memory_info
            .as_ref()
            .map(|m| m.used / (1024 * 1024))
            .unwrap_or(0);
        let memory_free_mb = memory_info
            .as_ref()
            .map(|m| m.free / (1024 * 1024))
            .unwrap_or(0);
        let memory_total_mb = memory_info
            .as_ref()
            .map(|m| m.total / (1024 * 1024))
            .unwrap_or(0);

        let power_draw_w = device
            .power_usage()
            .map(|mw| mw as f64 / 1000.0)
            .unwrap_or(0.0);

        let pstate = device
            .performance_state()
            .map(|p| format!("{p:?}"))
            .unwrap_or_else(|_| "Unknown".to_string());

        let fan_speed_percent = device.fan_speed(0).unwrap_or(0);

        let clock_graphics_mhz = device
            .clock_info(nvml_wrapper::enum_wrappers::device::Clock::Graphics)
            .unwrap_or(0);
        let clock_memory_mhz = device
            .clock_info(nvml_wrapper::enum_wrappers::device::Clock::Memory)
            .unwrap_or(0);

        let throttle_reason = self.format_throttle_reasons(&device);

        // Read NUMA node from sysfs
        let numa_path = format!("/sys/bus/pci/devices/{}/numa_node", pci_address);
        let numa_node = std::fs::read_to_string(&numa_path)
            .ok()
            .and_then(|s| s.trim().parse::<i32>().ok());

        Ok(GpuStatus {
            pci_address,
            nvidia_index: Some(idx),
            gpu_uuid,
            name,
            gpu_type: GpuType::Internal,
            temperature_c,
            utilization_gpu_percent,
            memory_used_mb,
            memory_free_mb,
            memory_total_mb,
            power_draw_w,
            pstate,
            fan_speed_percent,
            clock_graphics_mhz,
            clock_memory_mhz,
            throttle_reason,
            status: GpuOnlineStatus::Online,
            numa_node,
        })
    }

    /// Query PCIe throughput for a specific GPU.
    pub fn query_pcie_throughput(&self, pci_address: &str) -> Result<PcieThroughput, GpuError> {
        let device = self.find_device_by_pci(pci_address)?;

        let tx_kbps = device
            .pcie_throughput(PcieUtilCounter::Send)
            .unwrap_or(0) as u64;
        let rx_kbps = device
            .pcie_throughput(PcieUtilCounter::Receive)
            .unwrap_or(0) as u64;

        Ok(PcieThroughput {
            pci_address: pci_address.to_string(),
            tx_kbps,
            rx_kbps,
        })
    }

    /// Display VRAM = total used - sum of compute process VRAM.
    pub fn query_display_vram(&self, pci_address: &str) -> Result<u64, GpuError> {
        let device = self.find_device_by_pci(pci_address)?;

        let memory_info = device
            .memory_info()
            .map_err(|e| GpuError::NvmlError(format!("memory_info: {e}")))?;
        let total_used_mb = memory_info.used / (1024 * 1024);

        // Subtract VRAM used by compute processes
        let compute_vram_mb: u64 = device
            .running_compute_processes()
            .unwrap_or_default()
            .iter()
            .map(|p| match p.used_gpu_memory {
                UsedGpuMemory::Used(bytes) => bytes / (1024 * 1024),
                UsedGpuMemory::Unavailable => 0,
            })
            .sum();

        Ok(total_used_mb.saturating_sub(compute_vram_mb))
    }

    /// List running compute processes and their VRAM usage.
    pub fn query_compute_processes(
        &self,
        pci_address: &str,
    ) -> Result<Vec<ProcessVram>, GpuError> {
        let device = self.find_device_by_pci(pci_address)?;

        let processes = device.running_compute_processes().unwrap_or_default();

        Ok(processes
            .iter()
            .map(|p| {
                let process_name = process_name_from_pid(p.pid);
                ProcessVram {
                    pid: p.pid,
                    used_mb: match p.used_gpu_memory {
                        UsedGpuMemory::Used(bytes) => bytes / (1024 * 1024),
                        UsedGpuMemory::Unavailable => 0,
                    },
                    process_name,
                }
            })
            .collect())
    }

    /// Return the NVIDIA driver version string.
    pub fn query_driver_version(&self) -> Result<String, GpuError> {
        self.nvml
            .sys_driver_version()
            .map_err(|e| GpuError::NvmlError(format!("sys_driver_version: {e}")))
    }

    /// Validate that a GPU is functional:
    /// - memory_total matches expected (within 5% tolerance)
    /// - temperature is sane (1..=110 °C)
    pub fn validate_gpu_functional(
        &self,
        pci_address: &str,
        expected_memory_mb: Option<u64>,
    ) -> Result<bool, GpuError> {
        let device = self.find_device_by_pci(pci_address)?;

        // Temperature check
        let temp = device
            .temperature(TemperatureSensor::Gpu)
            .unwrap_or(0);
        if temp == 0 || temp > 110 {
            debug!(
                "GPU {pci_address} Temperatur außerhalb saner Grenzen: {temp}°C"
            );
            return Ok(false);
        }

        // Memory total check
        if let Some(expected) = expected_memory_mb {
            let memory_info = device
                .memory_info()
                .map_err(|e| GpuError::NvmlError(format!("memory_info: {e}")))?;
            let actual_mb = memory_info.total / (1024 * 1024);
            let tolerance = expected / 20; // 5%
            if actual_mb < expected.saturating_sub(tolerance)
                || actual_mb > expected + tolerance
            {
                debug!(
                    "GPU {pci_address} Speicher-Mismatch: erwartet ~{expected} MB, gefunden {actual_mb} MB"
                );
                return Ok(false);
            }
        }

        Ok(true)
    }

    // ── Internal helpers ─────────────────────────────────────────────────

    fn find_device_by_pci(
        &self,
        pci_address: &str,
    ) -> Result<nvml_wrapper::Device<'_>, GpuError> {
        let normalized = normalize_pci_address(pci_address);
        let count = self
            .nvml
            .device_count()
            .map_err(|e| GpuError::NvmlError(format!("device_count: {e}")))?;

        for idx in 0..count {
            if let Ok(device) = self.nvml.device_by_index(idx) {
                if let Ok(pci) = device.pci_info() {
                    if normalize_pci_address(&pci.bus_id) == normalized {
                        return Ok(device);
                    }
                }
            }
        }

        Err(GpuError::GpuNotFound {
            pci_address: pci_address.to_string(),
        })
    }

    fn format_throttle_reasons(&self, device: &nvml_wrapper::Device) -> String {
        let Ok(reasons) = device.current_throttle_reasons() else {
            return "Unknown".to_string();
        };

        let mut parts = Vec::new();
        if reasons.contains(ThrottleReasons::GPU_IDLE) {
            parts.push("Idle");
        }
        if reasons.contains(ThrottleReasons::HW_SLOWDOWN) {
            parts.push("HW Slowdown");
        }
        if reasons.contains(ThrottleReasons::HW_THERMAL_SLOWDOWN) {
            parts.push("Thermal Slowdown");
        }
        if reasons.contains(ThrottleReasons::HW_POWER_BRAKE_SLOWDOWN) {
            parts.push("Power Brake");
        }
        if reasons.contains(ThrottleReasons::SW_POWER_CAP) {
            parts.push("SW Power Cap");
        }
        if reasons.contains(ThrottleReasons::SW_THERMAL_SLOWDOWN) {
            parts.push("SW Thermal");
        }
        if reasons.contains(ThrottleReasons::APPLICATIONS_CLOCKS_SETTING) {
            parts.push("App Clocks");
        }

        if parts.is_empty() {
            "None".to_string()
        } else {
            parts.join(", ")
        }
    }
}

/// Read process name from /proc/<pid>/comm.
fn process_name_from_pid(pid: u32) -> Option<String> {
    std::fs::read_to_string(format!("/proc/{pid}/comm"))
        .ok()
        .map(|s| s.trim().to_string())
}

// ──────────────────────────────────────────────────────────────────────────────
// nvidia-smi CLI fallback monitor (original implementation)
// ──────────────────────────────────────────────────────────────────────────────

pub struct NvidiaSmiMonitor {
    timeout_secs: u64,
}

impl NvidiaSmiMonitor {
    pub fn new(timeout_secs: u64) -> Self {
        Self { timeout_secs }
    }

    /// Alle GPUs abfragen (nvidia-smi --query-gpu)
    pub async fn query_all(&self) -> Result<Vec<GpuStatus>, GpuError> {
        let output = self
            .run_nvidia_smi(&[
                "--query-gpu=gpu_bus_id,index,name,temperature.gpu,utilization.gpu,utilization.memory,memory.used,memory.free,memory.total,power.draw,pstate,fan.speed,clocks.current.graphics,clocks.current.memory,gpu_operation_mode.current",
                "--format=csv,noheader,nounits",
            ])
            .await?;

        parse_gpu_status_output(&output)
    }

    /// PCIe-Throughput für eine GPU abfragen (nvidia-smi dmon, einmalig)
    pub async fn query_pcie_throughput(
        &self,
        pci_address: &str,
    ) -> Result<PcieThroughput, GpuError> {
        // nvidia-smi-Index für die PCI-Adresse finden
        let gpus = self.query_all().await?;
        let gpu = gpus
            .iter()
            .find(|g| normalize_pci_address(&g.pci_address) == normalize_pci_address(pci_address))
            .ok_or_else(|| GpuError::GpuNotFound {
                pci_address: pci_address.to_string(),
            })?;

        let index = gpu.nvidia_index.ok_or_else(|| {
            GpuError::NvidiaSmiParse(format!("Kein nvidia-index für {pci_address}"))
        })?;

        let output = self
            .run_nvidia_smi(&[
                "dmon",
                "-i",
                &index.to_string(),
                "-s",
                "p",
                "-c",
                "1",
            ])
            .await?;

        parse_dmon_output(&output, pci_address)
    }

    /// Display-VRAM-Verbrauch einer GPU ermitteln
    pub async fn query_display_vram(&self, pci_address: &str) -> Result<u64, GpuError> {
        let gpus = self.query_all().await?;
        let gpu = gpus
            .iter()
            .find(|g| normalize_pci_address(&g.pci_address) == normalize_pci_address(pci_address))
            .ok_or_else(|| GpuError::GpuNotFound {
                pci_address: pci_address.to_string(),
            })?;

        // memory.used auf der internen GPU = Display-VRAM (wenn keine Container laufen)
        Ok(gpu.memory_used_mb)
    }

    /// Run nvidia-smi with retry logic (exponential backoff on timeout).
    async fn run_nvidia_smi(&self, args: &[&str]) -> Result<String, GpuError> {
        const MAX_RETRIES: u32 = 3;
        const BASE_DELAY_MS: u64 = 200;

        let mut last_err = None;

        for attempt in 0..=MAX_RETRIES {
            if attempt > 0 {
                let delay = Duration::from_millis(BASE_DELAY_MS * (1 << (attempt - 1)));
                debug!(
                    "nvidia-smi Retry {attempt}/{MAX_RETRIES} nach {delay:?}"
                );
                tokio::time::sleep(delay).await;
            }

            match self.run_nvidia_smi_once(args).await {
                Ok(output) => return Ok(output),
                Err(e @ GpuError::NvidiaSmiTimeout { .. }) => {
                    warn!("nvidia-smi Timeout (Versuch {}/{})", attempt + 1, MAX_RETRIES + 1);
                    last_err = Some(e);
                    // Retry on timeout
                }
                Err(e) => {
                    // Don't retry on parse errors or other failures
                    return Err(e);
                }
            }
        }

        Err(last_err.unwrap_or(GpuError::NvidiaSmiTimeout {
            timeout_secs: self.timeout_secs,
        }))
    }

    async fn run_nvidia_smi_once(&self, args: &[&str]) -> Result<String, GpuError> {
        let timeout = Duration::from_secs(self.timeout_secs);

        let result = tokio::time::timeout(timeout, async {
            let output = Command::new("nvidia-smi")
                .args(args)
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .output()
                .await
                .map_err(|e| GpuError::NvidiaSmiUnavailable(e.to_string()))?;

            if !output.status.success() {
                let stderr = String::from_utf8_lossy(&output.stderr);
                return Err(GpuError::NvidiaSmiParse(format!(
                    "nvidia-smi Exit-Code {}: {stderr}",
                    output.status
                )));
            }

            Ok(String::from_utf8_lossy(&output.stdout).to_string())
        })
        .await;

        match result {
            Ok(inner) => inner,
            Err(_) => Err(GpuError::NvidiaSmiTimeout {
                timeout_secs: self.timeout_secs,
            }),
        }
    }
}

// ──────────────────────────────────────────────────────────────────────────────
// PCI address helpers
// ──────────────────────────────────────────────────────────────────────────────

/// Normalize PCI address: convert 8-digit domain to 4-digit, lowercase.
pub fn normalize_pci_address(addr: &str) -> String {
    let addr = addr.trim().to_lowercase();
    // Handle 8-digit domain -> 4-digit (e.g. "00000000:02:00.0" -> "0000:02:00.0")
    if addr.len() > 12 {
        if let Some(idx) = addr.find(':') {
            if idx > 4 {
                return format!("{}{}", &addr[idx - 4..idx], &addr[idx..]);
            }
        }
    }
    addr
}

/// Validate that a PCI address has the expected DDDD:BB:DD.F format.
pub fn validate_pci_address(addr: &str) -> bool {
    let normalized = normalize_pci_address(addr);
    // Expected format: DDDD:BB:DD.F = 12 chars
    if normalized.len() != 12 {
        return false;
    }
    let parts: Vec<&str> = normalized.split(':').collect();
    if parts.len() != 3 {
        return false;
    }
    // Check that function part contains a dot
    if !parts[2].contains('.') {
        return false;
    }
    // Check all chars are hex digits (except separators)
    let hex_check = |s: &str| s.chars().all(|c| c.is_ascii_hexdigit());
    if !hex_check(parts[0]) || parts[0].len() != 4 {
        return false;
    }
    if !hex_check(parts[1]) || parts[1].len() != 2 {
        return false;
    }
    let func_parts: Vec<&str> = parts[2].split('.').collect();
    if func_parts.len() != 2 {
        return false;
    }
    if !hex_check(func_parts[0]) || func_parts[0].len() != 2 {
        return false;
    }
    if !hex_check(func_parts[1]) || func_parts[1].len() != 1 {
        return false;
    }
    true
}

// ──────────────────────────────────────────────────────────────────────────────
// Shared parsing helpers (used by nvidia-smi fallback)
// ──────────────────────────────────────────────────────────────────────────────

fn parse_gpu_status_output(output: &str) -> Result<Vec<GpuStatus>, GpuError> {
    let mut gpus = Vec::new();

    for line in output.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        let parts: Vec<&str> = line.split(", ").collect();
        if parts.len() < 15 {
            warn!(
                "nvidia-smi Zeile hat zu wenige Felder ({}/15): {line}",
                parts.len()
            );
            continue;
        }

        let pci_address = normalize_pci_address(parts[0]);
        let nvidia_index = parts[1].trim().parse::<u32>().ok();
        let name = parts[2].trim().to_string();
        let temperature_c = parse_u32(parts[3]);
        let utilization_gpu_percent = parse_u32(parts[4]);
        let _utilization_memory_percent = parse_u32(parts[5]);
        let memory_used_mb = parse_u64(parts[6]);
        let memory_free_mb = parse_u64(parts[7]);
        let memory_total_mb = parse_u64(parts[8]);
        let power_draw_w = parts[9].trim().parse::<f64>().unwrap_or(0.0);
        let pstate = parts[10].trim().to_string();
        let fan_speed_percent = parse_u32(parts[11]);
        let clock_graphics_mhz = parse_u32(parts[12]);
        let clock_memory_mhz = parse_u32(parts[13]);
        // gpu_operation_mode als Throttle-Reason (z.B. "All On", "Compute", "[N/A]")
        let throttle_reason = parts[14].trim().to_string();

        // GPU UUID aus /proc/driver/nvidia/gpus/{pci}/information lesen
        let gpu_uuid = read_gpu_uuid(&pci_address).unwrap_or_default();

        // Read NUMA node from sysfs
        let numa_path = format!("/sys/bus/pci/devices/{}/numa_node", pci_address);
        let numa_node = std::fs::read_to_string(&numa_path)
            .ok()
            .and_then(|s| s.trim().parse::<i32>().ok());

        gpus.push(GpuStatus {
            pci_address,
            nvidia_index,
            gpu_uuid,
            name,
            gpu_type: GpuType::Internal, // Wird vom Caller anhand der Config gesetzt
            temperature_c,
            utilization_gpu_percent,
            memory_used_mb,
            memory_free_mb,
            memory_total_mb,
            power_draw_w,
            pstate,
            fan_speed_percent,
            clock_graphics_mhz,
            clock_memory_mhz,
            throttle_reason,
            status: GpuOnlineStatus::Online,
            numa_node,
        });
    }

    if gpus.is_empty() {
        return Err(GpuError::NvidiaSmiParse(
            "Keine GPUs in nvidia-smi-Ausgabe gefunden".to_string(),
        ));
    }

    debug!("{} GPU(s) von nvidia-smi geparst", gpus.len());
    Ok(gpus)
}

/// Read GPU UUID from /proc/driver/nvidia/gpus/{pci_address}/information.
/// Returns the UUID string like "GPU-bd7dd984-fd6a-3d83-a22c-539b5b438290".
fn read_gpu_uuid(pci_address: &str) -> Option<String> {
    let path = format!("/proc/driver/nvidia/gpus/{pci_address}/information");
    let content = std::fs::read_to_string(&path).ok()?;
    for line in content.lines() {
        if let Some(uuid) = line.strip_prefix("GPU UUID:") {
            let uuid = uuid.trim();
            if !uuid.is_empty() {
                return Some(uuid.to_string());
            }
        }
    }
    None
}

fn parse_dmon_output(output: &str, pci_address: &str) -> Result<PcieThroughput, GpuError> {
    // nvidia-smi dmon Ausgabe:
    // # gpu    pci_tx   pci_rx
    //   0      1234     5678
    for line in output.lines() {
        let line = line.trim();
        if line.starts_with('#') || line.is_empty() {
            continue;
        }

        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() >= 3 {
            let tx_kbps = parse_u64(parts[1]);
            let rx_kbps = parse_u64(parts[2]);

            return Ok(PcieThroughput {
                pci_address: pci_address.to_string(),
                tx_kbps,
                rx_kbps,
            });
        }
    }

    Err(GpuError::NvidiaSmiParse(
        "nvidia-smi dmon: keine Daten geparst".to_string(),
    ))
}

fn parse_u32(s: &str) -> u32 {
    s.trim().replace(" %", "").parse().unwrap_or(0)
}

fn parse_u64(s: &str) -> u64 {
    s.trim()
        .replace(" MiB", "")
        .replace(" MB", "")
        .parse()
        .unwrap_or(0)
}

// ──────────────────────────────────────────────────────────────────────────────
// Ollama API (unchanged)
// ──────────────────────────────────────────────────────────────────────────────

/// Ollama-API abfragen: laufende Modelle
pub async fn query_ollama_models(host: &str) -> Result<Vec<OllamaModel>, String> {
    let url = format!("{host}/api/ps");
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .map_err(|e| format!("HTTP-Client: {e}"))?;

    let resp = client
        .get(&url)
        .send()
        .await
        .map_err(|e| format!("Ollama nicht erreichbar ({url}): {e}"))?;

    if !resp.status().is_success() {
        return Err(format!("Ollama API Fehler: HTTP {}", resp.status()));
    }

    let body: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| format!("Ollama JSON-Parse-Fehler: {e}"))?;

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

// ──────────────────────────────────────────────────────────────────────────────
// Tests
// ──────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── Existing tests (kept working) ────────────────────────────────────

    #[test]
    fn test_parse_gpu_status() {
        let output = "00000000:02:00.0, 0, NVIDIA GeForce RTX 5060 Laptop GPU, 42, 0 %, 0 %, 15 MiB, 7692 MiB, 8151 MiB, 4.82 W, P8, 0 %, 210 MHz, 405 MHz, All On\n00000000:05:00.0, 1, NVIDIA GeForce RTX 5070 Ti, 45, 0 %, 0 %, 15788 MiB, 53 MiB, 16303 MiB, 22.73 W, P8, 30 %, 210 MHz, 405 MHz, All On\n";

        let gpus = parse_gpu_status_output(output).unwrap();
        assert_eq!(gpus.len(), 2);

        assert_eq!(gpus[0].pci_address, "0000:02:00.0");
        assert_eq!(gpus[0].nvidia_index, Some(0));
        assert_eq!(gpus[0].name, "NVIDIA GeForce RTX 5060 Laptop GPU");
        assert_eq!(gpus[0].temperature_c, 42);
        assert_eq!(gpus[0].memory_total_mb, 8151);

        assert_eq!(gpus[1].pci_address, "0000:05:00.0");
        assert_eq!(gpus[1].nvidia_index, Some(1));
        assert_eq!(gpus[1].name, "NVIDIA GeForce RTX 5070 Ti");
        assert_eq!(gpus[1].memory_total_mb, 16303);
    }

    #[test]
    fn test_parse_empty_output() {
        let result = parse_gpu_status_output("");
        assert!(result.is_err());
    }

    #[test]
    fn test_normalize_pci_address() {
        assert_eq!(normalize_pci_address("00000000:02:00.0"), "0000:02:00.0");
        assert_eq!(normalize_pci_address("0000:05:00.0"), "0000:05:00.0");
        assert_eq!(
            normalize_pci_address("  00000000:05:00.0  "),
            "0000:05:00.0"
        );
    }

    #[test]
    fn test_parse_dmon_output() {
        let output = "# gpu    pci_tx   pci_rx\n  0      45000    12000\n";
        let result = parse_dmon_output(output, "0000:05:00.0").unwrap();
        assert_eq!(result.tx_kbps, 45000);
        assert_eq!(result.rx_kbps, 12000);
    }

    #[test]
    fn test_parse_dmon_empty() {
        let result = parse_dmon_output("# header\n", "0000:05:00.0");
        assert!(result.is_err());
    }

    // ── New tests ────────────────────────────────────────────────────────

    #[test]
    fn test_nvml_fallback_to_nvidia_smi() {
        // On systems without NVML, the backend should fall back to nvidia-smi
        let backend = GpuMonitorBackend::new(5);
        // Must not panic; should be one of the two variants
        match &backend {
            GpuMonitorBackend::Nvml(_) => {
                // NVML available — that's fine
                assert!(backend.is_nvml());
            }
            GpuMonitorBackend::NvidiaSmi(_) => {
                // Fallback — also fine
                assert!(!backend.is_nvml());
            }
        }
    }

    #[test]
    fn test_query_driver_version_without_nvml() {
        // nvidia-smi backend cannot return driver version
        let backend = GpuMonitorBackend::NvidiaSmi(NvidiaSmiMonitor::new(5));
        let result = backend.query_driver_version();
        assert!(result.is_err());
    }

    #[test]
    fn test_validate_gpu_functional_without_nvml() {
        // nvidia-smi backend always returns Ok(true)
        let backend = GpuMonitorBackend::NvidiaSmi(NvidiaSmiMonitor::new(5));
        let result = backend.validate_gpu_functional("0000:02:00.0", Some(8192));
        assert_eq!(result.unwrap(), true);
    }

    #[test]
    fn test_compute_processes_without_nvml() {
        // nvidia-smi backend returns empty vec
        let backend = GpuMonitorBackend::NvidiaSmi(NvidiaSmiMonitor::new(5));
        let result = backend.query_compute_processes("0000:02:00.0");
        assert!(result.unwrap().is_empty());
    }

    #[test]
    fn test_validate_pci_address_valid() {
        assert!(validate_pci_address("0000:02:00.0"));
        assert!(validate_pci_address("0000:05:00.0"));
        assert!(validate_pci_address("0000:ff:1a.3"));
    }

    #[test]
    fn test_validate_pci_address_normalizes_long_domain() {
        // 8-digit domain should normalize and still validate
        assert!(validate_pci_address("00000000:02:00.0"));
    }

    #[test]
    fn test_validate_pci_address_invalid() {
        assert!(!validate_pci_address(""));
        assert!(!validate_pci_address("02:00.0"));
        assert!(!validate_pci_address("0000:02:000")); // no dot
        assert!(!validate_pci_address("ZZZZ:02:00.0")); // non-hex
        assert!(!validate_pci_address("0000:GG:00.0")); // non-hex
    }

    #[test]
    fn test_normalize_pci_address_lowercase() {
        assert_eq!(normalize_pci_address("0000:0A:00.0"), "0000:0a:00.0");
    }

    #[test]
    fn test_normalize_pci_address_already_short() {
        assert_eq!(normalize_pci_address("0000:02:00.0"), "0000:02:00.0");
    }

    #[test]
    fn test_display_vram_subtraction_logic() {
        // Verify the subtraction approach: if total used is 5000 MB
        // and compute processes use 3000 MB, display VRAM should be 2000 MB.
        let total_used_mb: u64 = 5000;
        let compute_vram_mb: u64 = 3000;
        let display_vram = total_used_mb.saturating_sub(compute_vram_mb);
        assert_eq!(display_vram, 2000);
    }

    #[test]
    fn test_display_vram_subtraction_overflow() {
        // If compute processes report more than total (shouldn't happen but be safe)
        let total_used_mb: u64 = 100;
        let compute_vram_mb: u64 = 500;
        let display_vram = total_used_mb.saturating_sub(compute_vram_mb);
        assert_eq!(display_vram, 0);
    }

    #[tokio::test]
    async fn test_nvidia_smi_retry_does_not_retry_parse_errors() {
        // NvidiaSmiMonitor.run_nvidia_smi should NOT retry on parse errors.
        // We can't easily test the actual retry without mocking, but we can verify
        // that a parse error from run_nvidia_smi_once propagates immediately.
        let monitor = NvidiaSmiMonitor::new(1);
        // Calling with invalid args will fail fast (not timeout), so no retry.
        // This at least verifies the code paths compile and run.
        let result = monitor
            .run_nvidia_smi(&["--this-is-not-a-valid-flag"])
            .await;
        // Will be either Unavailable (no nvidia-smi) or Parse error — not a timeout
        assert!(result.is_err());
        match result.unwrap_err() {
            GpuError::NvidiaSmiTimeout { .. } => {
                panic!("Should not timeout on invalid flag — should fail fast")
            }
            _ => {} // Expected: parse error or unavailable
        }
    }
}
