//! Prometheus-Metriken fuer den eGPU Manager Daemon.
//!
//! Exportiert GPU-Telemetrie, Scheduler-Zustand, Warning-Level und
//! Daemon-Health als Prometheus text-format ueber GET /metrics.

use std::sync::Arc;

use prometheus_client::encoding::text::encode;
use prometheus_client::metrics::counter::Counter;
use prometheus_client::metrics::family::Family;
use prometheus_client::metrics::gauge::Gauge;
use prometheus_client::metrics::histogram::{exponential_buckets, Histogram};
use prometheus_client::registry::Registry;
use tokio::sync::Mutex;

/// Labels fuer GPU-bezogene Metriken.
#[derive(Clone, Debug, Hash, PartialEq, Eq, prometheus_client::encoding::EncodeLabelSet)]
pub struct GpuLabels {
    pub gpu: String,
}

/// Labels fuer LLM Gateway-Metriken (pro App).
#[derive(Clone, Debug, Hash, PartialEq, Eq, prometheus_client::encoding::EncodeLabelSet)]
pub struct GatewayLabels {
    pub app_id: String,
}

/// Labels fuer LLM Gateway-Metriken (pro App + Provider).
#[derive(Clone, Debug, Hash, PartialEq, Eq, prometheus_client::encoding::EncodeLabelSet)]
pub struct GatewayProviderLabels {
    pub app_id: String,
    pub provider: String,
}

/// Alle Prometheus-Metriken des Daemons.
#[allow(dead_code)] // PCIe-Throughput-Felder registriert aber noch nicht beschrieben
pub struct DaemonMetrics {
    // GPU-Telemetrie (pro GPU)
    pub gpu_temperature_celsius: Family<GpuLabels, Gauge>,
    pub gpu_utilization_percent: Family<GpuLabels, Gauge>,
    pub gpu_vram_used_mb: Family<GpuLabels, Gauge>,
    pub gpu_vram_total_mb: Family<GpuLabels, Gauge>,
    pub gpu_vram_available_mb: Family<GpuLabels, Gauge>,
    pub gpu_power_draw_watts: Family<GpuLabels, Gauge<f64, AtomicU64>>,
    pub gpu_clock_graphics_mhz: Family<GpuLabels, Gauge>,
    pub gpu_clock_memory_mhz: Family<GpuLabels, Gauge>,

    // PCIe
    pub pcie_throughput_tx_kbps: Family<GpuLabels, Gauge>,
    pub pcie_throughput_rx_kbps: Family<GpuLabels, Gauge>,

    // Scheduler
    pub scheduler_assignments_total: Family<GpuLabels, Gauge>,
    pub scheduler_queue_length: Gauge,
    pub scheduler_vram_used_mb: Family<GpuLabels, Gauge>,
    pub scheduler_display_reserve_mb: Gauge,

    // Daemon-Zustand
    pub warning_level: Gauge,
    pub health_score: Gauge<f64, AtomicU64>,
    pub active_leases_total: Gauge,
    pub recovery_active: Gauge,

    // Zaehler
    pub nvidia_smi_timeouts_total: Counter,
    pub aer_errors_total: Counter,
    pub xid_errors_total: Counter,
    pub recovery_stages_total: Counter,

    // Histogramm
    pub nvidia_query_duration_ms: Histogram,

    // ─── LLM Gateway Metriken ────────────────────────────────────────
    /// Chat-Completion-Requests pro App
    pub gateway_chat_requests_total: Family<GatewayLabels, Counter>,
    /// Embedding-Requests pro App
    pub gateway_embedding_requests_total: Family<GatewayLabels, Counter>,
    /// Gateway-Fehler pro App + Provider
    pub gateway_errors_total: Family<GatewayProviderLabels, Counter>,
    /// Aktive Staging-Leases
    pub gateway_staging_leases_active: Gauge,
    /// Gateway Chat-Latenz in Millisekunden
    pub gateway_chat_latency_ms: Histogram,
    /// Gateway Embedding-Latenz in Millisekunden
    pub gateway_embedding_latency_ms: Histogram,
    /// Tokens verarbeitet pro App (Input + Output)
    pub gateway_tokens_total: Family<GatewayLabels, Counter>,
}

use std::sync::atomic::AtomicU64;

impl DaemonMetrics {
    pub fn new() -> (Self, Registry) {
        let mut registry = Registry::default();

        let gpu_temperature_celsius = Family::<GpuLabels, Gauge>::default();
        registry.register(
            "egpu_gpu_temperature_celsius",
            "GPU-Temperatur in Grad Celsius",
            gpu_temperature_celsius.clone(),
        );

        let gpu_utilization_percent = Family::<GpuLabels, Gauge>::default();
        registry.register(
            "egpu_gpu_utilization_percent",
            "GPU-Auslastung in Prozent",
            gpu_utilization_percent.clone(),
        );

        let gpu_vram_used_mb = Family::<GpuLabels, Gauge>::default();
        registry.register(
            "egpu_gpu_vram_used_mb",
            "VRAM belegt in MB",
            gpu_vram_used_mb.clone(),
        );

        let gpu_vram_total_mb = Family::<GpuLabels, Gauge>::default();
        registry.register(
            "egpu_gpu_vram_total_mb",
            "VRAM gesamt in MB",
            gpu_vram_total_mb.clone(),
        );

        let gpu_vram_available_mb = Family::<GpuLabels, Gauge>::default();
        registry.register(
            "egpu_gpu_vram_available_mb",
            "VRAM verfuegbar in MB (nach Scheduler-Reserve)",
            gpu_vram_available_mb.clone(),
        );

        let gpu_power_draw_watts = Family::<GpuLabels, Gauge<f64, AtomicU64>>::default();
        registry.register(
            "egpu_gpu_power_draw_watts",
            "GPU-Leistungsaufnahme in Watt",
            gpu_power_draw_watts.clone(),
        );

        let gpu_clock_graphics_mhz = Family::<GpuLabels, Gauge>::default();
        registry.register(
            "egpu_gpu_clock_graphics_mhz",
            "GPU Graphics-Clock in MHz",
            gpu_clock_graphics_mhz.clone(),
        );

        let gpu_clock_memory_mhz = Family::<GpuLabels, Gauge>::default();
        registry.register(
            "egpu_gpu_clock_memory_mhz",
            "GPU Memory-Clock in MHz",
            gpu_clock_memory_mhz.clone(),
        );

        let pcie_throughput_tx_kbps = Family::<GpuLabels, Gauge>::default();
        registry.register(
            "egpu_pcie_throughput_tx_kbps",
            "PCIe TX-Durchsatz in KB/s",
            pcie_throughput_tx_kbps.clone(),
        );

        let pcie_throughput_rx_kbps = Family::<GpuLabels, Gauge>::default();
        registry.register(
            "egpu_pcie_throughput_rx_kbps",
            "PCIe RX-Durchsatz in KB/s",
            pcie_throughput_rx_kbps.clone(),
        );

        let scheduler_assignments_total = Family::<GpuLabels, Gauge>::default();
        registry.register(
            "egpu_scheduler_assignments_total",
            "Anzahl Scheduler-Zuweisungen pro GPU",
            scheduler_assignments_total.clone(),
        );

        let scheduler_queue_length = Gauge::default();
        registry.register(
            "egpu_scheduler_queue_length",
            "Laenge der Scheduler-Warteschlange",
            scheduler_queue_length.clone(),
        );

        let scheduler_vram_used_mb = Family::<GpuLabels, Gauge>::default();
        registry.register(
            "egpu_scheduler_vram_used_mb",
            "Scheduler: verplantes VRAM pro GPU in MB",
            scheduler_vram_used_mb.clone(),
        );

        let scheduler_display_reserve_mb = Gauge::default();
        registry.register(
            "egpu_scheduler_display_reserve_mb",
            "Display-VRAM-Reserve der internen GPU in MB",
            scheduler_display_reserve_mb.clone(),
        );

        let warning_level = Gauge::default();
        registry.register(
            "egpu_warning_level",
            "Aktuelle Warnstufe (0=Green, 1=Yellow, 2=Orange, 3=Red)",
            warning_level.clone(),
        );

        let health_score = Gauge::<f64, AtomicU64>::default();
        registry.register(
            "egpu_health_score",
            "Link-Health-Score (0-100)",
            health_score.clone(),
        );

        let active_leases_total = Gauge::default();
        registry.register(
            "egpu_active_leases_total",
            "Anzahl aktiver GPU-Leases",
            active_leases_total.clone(),
        );

        let recovery_active = Gauge::default();
        registry.register(
            "egpu_recovery_active",
            "Recovery aktiv (0/1)",
            recovery_active.clone(),
        );

        let nvidia_smi_timeouts_total = Counter::default();
        registry.register(
            "egpu_nvidia_smi_timeouts_total",
            "Anzahl nvidia-smi Timeouts",
            nvidia_smi_timeouts_total.clone(),
        );

        let aer_errors_total = Counter::default();
        registry.register(
            "egpu_aer_errors_total",
            "Anzahl AER-Fehler (kumulativ)",
            aer_errors_total.clone(),
        );

        let xid_errors_total = Counter::default();
        registry.register(
            "egpu_xid_errors_total",
            "Anzahl NVIDIA Xid-Fehler (kumulativ)",
            xid_errors_total.clone(),
        );

        let recovery_stages_total = Counter::default();
        registry.register(
            "egpu_recovery_stages_total",
            "Anzahl durchlaufener Recovery-Stufen (kumulativ)",
            recovery_stages_total.clone(),
        );

        let nvidia_query_duration_ms =
            Histogram::new(exponential_buckets(1.0, 2.0, 12));
        registry.register(
            "egpu_nvidia_query_duration_ms",
            "Dauer einer nvidia-smi/NVML-Abfrage in Millisekunden",
            nvidia_query_duration_ms.clone(),
        );

        // ─── LLM Gateway Metriken ────────────────────────────────────────

        let gateway_chat_requests_total = Family::<GatewayLabels, Counter>::default();
        registry.register(
            "egpu_gateway_chat_requests_total",
            "Chat-Completion-Requests ueber das Gateway pro App",
            gateway_chat_requests_total.clone(),
        );

        let gateway_embedding_requests_total = Family::<GatewayLabels, Counter>::default();
        registry.register(
            "egpu_gateway_embedding_requests_total",
            "Embedding-Requests ueber das Gateway pro App",
            gateway_embedding_requests_total.clone(),
        );

        let gateway_errors_total = Family::<GatewayProviderLabels, Counter>::default();
        registry.register(
            "egpu_gateway_errors_total",
            "Gateway-Fehler pro App und Provider",
            gateway_errors_total.clone(),
        );

        let gateway_staging_leases_active = Gauge::default();
        registry.register(
            "egpu_gateway_staging_leases_active",
            "Anzahl aktiver Staging-Reservierungen",
            gateway_staging_leases_active.clone(),
        );

        let gateway_chat_latency_ms =
            Histogram::new(exponential_buckets(10.0, 2.0, 12));
        registry.register(
            "egpu_gateway_chat_latency_ms",
            "Chat-Completion Latenz in Millisekunden",
            gateway_chat_latency_ms.clone(),
        );

        let gateway_embedding_latency_ms =
            Histogram::new(exponential_buckets(5.0, 2.0, 12));
        registry.register(
            "egpu_gateway_embedding_latency_ms",
            "Embedding Latenz in Millisekunden",
            gateway_embedding_latency_ms.clone(),
        );

        let gateway_tokens_total = Family::<GatewayLabels, Counter>::default();
        registry.register(
            "egpu_gateway_tokens_total",
            "Tokens verarbeitet pro App (Input + Output)",
            gateway_tokens_total.clone(),
        );

        let metrics = Self {
            gpu_temperature_celsius,
            gpu_utilization_percent,
            gpu_vram_used_mb,
            gpu_vram_total_mb,
            gpu_vram_available_mb,
            gpu_power_draw_watts,
            gpu_clock_graphics_mhz,
            gpu_clock_memory_mhz,
            pcie_throughput_tx_kbps,
            pcie_throughput_rx_kbps,
            scheduler_assignments_total,
            scheduler_queue_length,
            scheduler_vram_used_mb,
            scheduler_display_reserve_mb,
            warning_level,
            health_score,
            active_leases_total,
            recovery_active,
            nvidia_smi_timeouts_total,
            aer_errors_total,
            xid_errors_total,
            recovery_stages_total,
            nvidia_query_duration_ms,
            gateway_chat_requests_total,
            gateway_embedding_requests_total,
            gateway_errors_total,
            gateway_staging_leases_active,
            gateway_chat_latency_ms,
            gateway_embedding_latency_ms,
            gateway_tokens_total,
        };

        (metrics, registry)
    }

    /// GPU-Telemetrie aktualisieren.
    pub fn update_gpu(&self, gpu_label: &str, temp: u32, util: u32, vram_used: u64, vram_total: u64, power: f64, clock_gfx: u32, clock_mem: u32) {
        let labels = GpuLabels { gpu: gpu_label.to_string() };
        self.gpu_temperature_celsius.get_or_create(&labels).set(temp as i64);
        self.gpu_utilization_percent.get_or_create(&labels).set(util as i64);
        self.gpu_vram_used_mb.get_or_create(&labels).set(vram_used as i64);
        self.gpu_vram_total_mb.get_or_create(&labels).set(vram_total as i64);
        self.gpu_power_draw_watts.get_or_create(&labels).set(power);
        self.gpu_clock_graphics_mhz.get_or_create(&labels).set(clock_gfx as i64);
        self.gpu_clock_memory_mhz.get_or_create(&labels).set(clock_mem as i64);
    }

    /// Scheduler-Metriken aktualisieren.
    pub fn update_scheduler(&self, gpu_label: &str, assignments: i64, vram_used: i64, vram_available: i64) {
        let labels = GpuLabels { gpu: gpu_label.to_string() };
        self.scheduler_assignments_total.get_or_create(&labels).set(assignments);
        self.scheduler_vram_used_mb.get_or_create(&labels).set(vram_used);
        self.gpu_vram_available_mb.get_or_create(&labels).set(vram_available);
    }

    /// Warning-Level als Zahl (0=Green, 1=Yellow, 2=Orange, 3=Red).
    pub fn set_warning_level(&self, level: i64) {
        self.warning_level.set(level);
    }

    /// Gateway Chat-Request erfassen.
    pub fn record_chat_request(&self, app_id: &str, latency_ms: f64, tokens: u64) {
        let labels = GatewayLabels { app_id: app_id.to_string() };
        self.gateway_chat_requests_total.get_or_create(&labels).inc();
        self.gateway_chat_latency_ms.observe(latency_ms);
        self.gateway_tokens_total.get_or_create(&labels).inc_by(tokens);
    }

    /// Gateway Embedding-Request erfassen.
    pub fn record_embedding_request(&self, app_id: &str, latency_ms: f64) {
        let labels = GatewayLabels { app_id: app_id.to_string() };
        self.gateway_embedding_requests_total.get_or_create(&labels).inc();
        self.gateway_embedding_latency_ms.observe(latency_ms);
    }

    /// Gateway-Fehler erfassen.
    pub fn record_gateway_error(&self, app_id: &str, provider: &str) {
        let labels = GatewayProviderLabels {
            app_id: app_id.to_string(),
            provider: provider.to_string(),
        };
        self.gateway_errors_total.get_or_create(&labels).inc();
    }
}

/// Shared Metrics-State fuer Web-Handler.
pub struct MetricsState {
    pub registry: Registry,
}

impl MetricsState {
    pub fn encode(&self) -> String {
        let mut buf = String::new();
        encode(&mut buf, &self.registry).unwrap_or_default();
        buf
    }
}

/// Erstelle Metriken und Registry.
pub fn create_metrics() -> (Arc<DaemonMetrics>, Arc<Mutex<MetricsState>>) {
    let (metrics, registry) = DaemonMetrics::new();
    let state = MetricsState { registry };
    (Arc::new(metrics), Arc::new(Mutex::new(state)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_metrics_creation() {
        let (metrics, _state) = create_metrics();
        // Initial values should be 0
        let labels = GpuLabels { gpu: "egpu".to_string() };
        metrics.gpu_temperature_celsius.get_or_create(&labels).set(45);
        // No panic = success
    }

    #[test]
    fn test_metrics_encode() {
        let (metrics, state) = create_metrics();
        metrics.update_gpu("egpu", 45, 80, 8000, 16000, 200.5, 2100, 10000);
        metrics.update_gpu("internal", 42, 10, 512, 8151, 5.0, 210, 405);

        let state_lock = state.blocking_lock();
        let output = state_lock.encode();
        assert!(output.contains("egpu_gpu_temperature_celsius"));
        assert!(output.contains("45"));
    }

    #[test]
    fn test_warning_level_gauge() {
        let (metrics, _state) = create_metrics();
        metrics.set_warning_level(2); // Orange
        // Counter/Gauge increments
        metrics.nvidia_smi_timeouts_total.inc();
        metrics.aer_errors_total.inc();
        metrics.xid_errors_total.inc();
    }

    #[test]
    fn test_histogram_observation() {
        let (metrics, _state) = create_metrics();
        metrics.nvidia_query_duration_ms.observe(42.5);
        metrics.nvidia_query_duration_ms.observe(150.0);
    }

    #[test]
    fn test_gateway_metrics() {
        let (metrics, state) = create_metrics();

        // Chat-Request simulieren
        metrics.record_chat_request("audit_designer", 250.0, 1500);
        metrics.record_chat_request("audit_designer", 180.0, 800);
        metrics.record_chat_request("flowinvoice", 300.0, 2000);

        // Embedding-Request simulieren
        metrics.record_embedding_request("audit_designer", 45.0);

        // Fehler simulieren
        metrics.record_gateway_error("flowinvoice", "ollama-egpu");

        // Staging
        metrics.gateway_staging_leases_active.set(2);

        let state_lock = state.blocking_lock();
        let output = state_lock.encode();
        assert!(output.contains("egpu_gateway_chat_requests_total"));
        assert!(output.contains("egpu_gateway_embedding_requests_total"));
        assert!(output.contains("egpu_gateway_errors_total"));
        assert!(output.contains("egpu_gateway_staging_leases_active"));
        assert!(output.contains("egpu_gateway_chat_latency_ms"));
        assert!(output.contains("egpu_gateway_tokens_total"));
        assert!(output.contains("audit_designer"));
        assert!(output.contains("flowinvoice"));
    }
}
