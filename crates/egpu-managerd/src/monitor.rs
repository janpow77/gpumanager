use std::collections::{HashMap, VecDeque};
use std::sync::Arc;

use chrono::{DateTime, Utc};
use egpu_manager_common::config::{Config, PipelineConfig, RemoteGpuConfig};
use egpu_manager_common::gpu::{GpuStatus, GpuType, OllamaModel, PcieThroughput, WarningLevel};
use egpu_manager_common::hal::{AerMonitor, OllamaControl, PcieLinkMonitor};
use serde::{Deserialize, Serialize};
use tokio::sync::{Mutex, mpsc};
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, warn};
use uuid::Uuid;

use crate::aer::AerWatcher;
use crate::db::{EventDb, Severity};
use crate::docker::DockerComposeControl;
use crate::health_score::{HealthEventKind, LinkHealthScore};
use crate::kmsg::KmsgMonitor;
use crate::link_health::LinkHealthWatcher;
use crate::nvidia::GpuMonitorBackend;
use crate::recovery::{RecoveryStateMachine, get_egpu_pipelines};
use crate::scheduler::{AdmissionState, GpuCapacity, GpuTarget, ScheduleRequest, VramScheduler};
use crate::sysfs::{SysfsAerMonitor, SysfsLinkMonitor};
use crate::warning::{WarningStateMachine, WarningTrigger};
use crate::web::sse::{BroadcastEvent, SseBroadcaster};

const EGPU_PROTECTION_HEADROOM_MB: u64 = 1024;

/// A GPU lease granted to an application.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LeaseTargetKind {
    Egpu,
    Internal,
    Remote,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GpuLease {
    pub lease_id: String,
    pub pipeline: String,
    pub gpu_device: String,
    pub gpu_uuid: String,
    pub vram_mb: u64,
    pub workload_type: String,
    pub acquired_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub target_kind: LeaseTargetKind,
    #[serde(default)]
    pub nvidia_index: Option<u32>,
    #[serde(default)]
    pub nvidia_visible_devices: Option<String>,
    #[serde(default)]
    pub assignment_source: String,
    #[serde(default)]
    pub remote_gpu_name: Option<String>,
    #[serde(default)]
    pub remote_host: Option<String>,
    #[serde(default)]
    pub remote_ollama_url: Option<String>,
    #[serde(default)]
    pub remote_agent_url: Option<String>,
    /// Last heartbeat timestamp for lease liveness detection.
    #[serde(default = "Utc::now")]
    pub last_heartbeat: DateTime<Utc>,
}

/// A registered remote GPU node (from LAN registration via port 7843).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegisteredRemoteGpu {
    pub name: String,
    pub host: String,
    pub port_ollama: u16,
    pub port_agent: u16,
    pub gpu_name: String,
    pub vram_mb: u64,
    pub status: String,
    pub last_heartbeat: DateTime<Utc>,
    pub latency_ms: Option<u32>,
}

/// Shared monitoring state accessible by API and other components.
pub struct MonitorState {
    pub warning_machine: WarningStateMachine,
    pub scheduler: VramScheduler,
    pub health_score: LinkHealthScore,
    pub gpu_status: Vec<GpuStatus>,
    pub pcie_throughput: HashMap<String, PcieThroughput>,
    pub ollama_models: Vec<OllamaModel>,
    /// Modelle pro Ollama-Instanz (Fleet-Modus)
    pub ollama_models_by_instance: HashMap<String, Vec<OllamaModel>>,
    pub active_leases: HashMap<String, GpuLease>,
    pub recovery_active: bool,
    pub remote_gpus: Vec<RegisteredRemoteGpu>,
}

impl MonitorState {
    /// Alle Ollama-Modelle (aus allen Instanzen) als flache Liste.
    pub fn all_ollama_models(&self) -> Vec<OllamaModel> {
        if !self.ollama_models_by_instance.is_empty() {
            self.ollama_models_by_instance
                .values()
                .flat_map(|v| v.iter().cloned())
                .collect()
        } else {
            self.ollama_models.clone()
        }
    }
}

/// The monitoring orchestrator. Spawns all monitoring tasks and
/// aggregates results into shared state.
pub struct MonitorOrchestrator {
    config: Arc<Config>,
    db: EventDb,
    state: Arc<Mutex<MonitorState>>,
    cancel: CancellationToken,
    sse: Option<SseBroadcaster>,
    metrics: Option<Arc<crate::metrics::DaemonMetrics>>,
}

#[derive(Debug, Clone)]
struct RemoteLeaseCandidate {
    name: String,
    host: String,
    port_ollama: u16,
    port_agent: u16,
    available_vram_mb: u64,
    latency_ms: Option<u32>,
    priority: u32,
}

#[derive(Debug, Clone)]
enum LeasePlacement {
    Local {
        target: GpuTarget,
        assignment_source: String,
    },
    Remote(RemoteLeaseCandidate),
}

#[derive(Debug, Clone)]
pub struct GpuRecommendation {
    pub recommended_gpu: String,
    pub recommended_device: String,
    pub assignment_source: String,
    pub target_kind: LeaseTargetKind,
    pub gpu_uuid: Option<String>,
    pub nvidia_index: Option<u32>,
    pub nvidia_visible_devices: Option<String>,
    pub remote_gpu_name: Option<String>,
    pub remote_host: Option<String>,
    pub remote_ollama_url: Option<String>,
    pub remote_agent_url: Option<String>,
    pub egpu_vram_available_mb: u64,
    pub internal_vram_available_mb: u64,
    pub remote_vram_available_mb: Option<u64>,
}

impl MonitorOrchestrator {
    pub fn new(config: Config, db: EventDb, cancel: CancellationToken) -> Self {
        let config = Arc::new(config);

        let warning_machine = WarningStateMachine::new(config.gpu.warning_cooldown_seconds);

        let scheduler = VramScheduler::new(
            GpuCapacity {
                total_vram_mb: 16000, // Will be updated from nvidia-smi
                display_reserve_mb: 0,
            },
            GpuCapacity {
                total_vram_mb: 8000, // Will be updated from nvidia-smi
                display_reserve_mb: config.gpu.display_vram_reserve_mb,
            },
            config.gpu.compute_warning_percent,
        );

        let health_score = LinkHealthScore::new(
            config.gpu.health_score_aer_penalty,
            config.gpu.health_score_pcie_error_penalty,
            config.gpu.health_score_smi_slow_penalty,
            config.gpu.health_score_thermal_penalty,
            config.gpu.health_score_recovery_per_minute,
            config.gpu.health_score_warning_threshold,
            config.gpu.health_score_critical_threshold,
        );

        let state = Arc::new(Mutex::new(MonitorState {
            warning_machine,
            scheduler,
            health_score,
            gpu_status: Vec::new(),
            pcie_throughput: HashMap::new(),
            ollama_models: Vec::new(),
            ollama_models_by_instance: HashMap::new(),
            active_leases: HashMap::new(),
            recovery_active: false,
            remote_gpus: Vec::new(),
        }));

        Self {
            config,
            db,
            state,
            cancel,
            sse: None,
            metrics: None,
        }
    }

    /// Set the SSE broadcaster for real-time event delivery.
    pub fn set_sse_broadcaster(&mut self, sse: SseBroadcaster) {
        self.sse = Some(sse);
    }

    /// Set the Prometheus metrics instance.
    pub fn set_metrics(&mut self, metrics: Arc<crate::metrics::DaemonMetrics>) {
        self.metrics = Some(metrics);
    }

    /// Get a reference to the shared monitoring state.
    pub fn state(&self) -> Arc<Mutex<MonitorState>> {
        Arc::clone(&self.state)
    }

    /// Start all monitoring tasks. Returns when cancelled.
    pub async fn run(&self) {
        info!("Monitoring-Orchestrator wird gestartet");

        // Channel for warning triggers from all monitoring tasks
        let (trigger_tx, mut trigger_rx) = mpsc::channel::<WarningTrigger>(64);

        // Spawn AER monitoring
        let aer_watcher = AerWatcher::new(
            self.config.gpu.egpu_pci_address.clone(),
            self.config.gpu.poll_interval_seconds,
            self.config.gpu.aer_warning_threshold,
            self.config.gpu.aer_burst_threshold,
            self.config.gpu.aer_window_seconds,
        );
        let aer_monitor: Arc<dyn AerMonitor> = Arc::new(SysfsAerMonitor);
        let aer_tx = trigger_tx.clone();
        let aer_cancel = self.cancel.clone();
        tokio::spawn(async move {
            aer_watcher.run(aer_monitor, aer_tx, aer_cancel).await;
        });

        // Spawn link health monitoring
        let link_watcher = LinkHealthWatcher::new(
            self.config.gpu.egpu_pci_address.clone(),
            self.config.gpu.link_health_check_interval_ms,
        );
        let link_monitor: Arc<dyn PcieLinkMonitor> = Arc::new(SysfsLinkMonitor);
        let link_tx = trigger_tx.clone();
        let link_cancel = self.cancel.clone();
        tokio::spawn(async move {
            link_watcher.run(link_monitor, link_tx, link_cancel).await;
        });

        // Spawn kmsg monitoring
        let kmsg_monitor = KmsgMonitor::new(self.config.gpu.egpu_pci_address.clone());
        let kmsg_tx = trigger_tx.clone();
        let kmsg_cancel = self.cancel.clone();
        tokio::spawn(async move {
            kmsg_monitor.run(kmsg_tx, kmsg_cancel).await;
        });

        // Spawn GPU polling loop (Gap 1)
        let gpu_state = Arc::clone(&self.state);
        let gpu_config = Arc::clone(&self.config);
        let gpu_cancel = self.cancel.clone();
        let gpu_sse = self.sse.clone();
        let gpu_trigger_tx = trigger_tx.clone();
        let gpu_db = self.db.clone();
        let gpu_metrics = self.metrics.clone();
        tokio::spawn(async move {
            Self::gpu_polling_loop(
                gpu_state,
                gpu_config,
                gpu_cancel,
                gpu_sse,
                gpu_trigger_tx,
                gpu_db,
                gpu_metrics,
            )
            .await;
        });

        // Spawn lease expiry loop
        let lease_state = Arc::clone(&self.state);
        let lease_cancel = self.cancel.clone();
        tokio::spawn(async move {
            Self::lease_expiry_loop(lease_state, lease_cancel).await;
        });

        // Spawn retention/aggregation task
        let db_clone = self.db.clone();
        let retention_config = Arc::clone(&self.config);
        let retention_cancel = self.cancel.clone();
        tokio::spawn(async move {
            Self::retention_loop(db_clone, retention_config, retention_cancel).await;
        });

        // FIX 7: Spawn CUDA watchdog task
        if self.config.gpu.cuda_watchdog_enabled {
            let watchdog_config = Arc::clone(&self.config);
            let watchdog_tx = trigger_tx.clone();
            let watchdog_cancel = self.cancel.clone();
            tokio::spawn(async move {
                Self::cuda_watchdog_loop(watchdog_config, watchdog_tx, watchdog_cancel).await;
            });
        }

        // Spawn step-down check task
        let state_clone = Arc::clone(&self.state);
        let db_clone2 = self.db.clone();
        let stepdown_cancel = self.cancel.clone();
        let stepdown_sse = self.sse.clone();
        tokio::spawn(async move {
            Self::stepdown_loop(state_clone, db_clone2, stepdown_cancel, stepdown_sse).await;
        });

        // Register configured pipelines in the scheduler
        {
            let mut state = self.state.lock().await;
            for pipeline in &self.config.pipeline {
                let preferred = if pipeline.gpu_device == self.config.gpu.egpu_pci_address {
                    GpuTarget::Egpu
                } else {
                    GpuTarget::Internal
                };
                state.scheduler.schedule(ScheduleRequest {
                    name: pipeline.container.clone(),
                    priority: pipeline.gpu_priority,
                    vram_estimate_mb: pipeline.vram_estimate_mb,
                    preferred_target: preferred,
                });
            }
        }

        // Drop our copy of trigger_tx so the channel closes when all producers stop
        drop(trigger_tx);

        // Main trigger processing loop
        info!("Trigger-Verarbeitung gestartet");
        loop {
            tokio::select! {
                _ = self.cancel.cancelled() => {
                    info!("Monitoring-Orchestrator wird beendet");
                    return;
                }
                trigger = trigger_rx.recv() => {
                    match trigger {
                        Some(trigger) => {
                            self.handle_trigger(trigger).await;
                        }
                        None => {
                            // All senders dropped (shouldn't happen unless cancelled)
                            info!("Alle Trigger-Sender beendet");
                            return;
                        }
                    }
                }
            }
        }
    }

    async fn handle_trigger(&self, trigger: WarningTrigger) {
        let trigger_str = trigger.to_string();
        let trigger_clone = trigger.clone();

        // M1: Collect data under lock, then drop before async I/O
        let level_change = {
            let mut state = self.state.lock().await;

            // FIX 12: Health-Score-Events bei JEDEM Trigger aufzeichnen, nicht nur bei Eskalation
            match &trigger_clone {
                t if *t == WarningTrigger::AerThreshold || *t == WarningTrigger::AerBurst => {
                    state.health_score.record_event(HealthEventKind::AerError);
                }
                t if *t == WarningTrigger::LinkWidthDegradation
                    || *t == WarningTrigger::LinkSpeedDegradation
                    || *t == WarningTrigger::LinkDown =>
                {
                    state
                        .health_score
                        .record_event(HealthEventKind::PcieTransient);
                }
                // FIX 11: CmpltToPattern und GpuProgressError ebenfalls als PCIe-Transient werten
                t if *t == WarningTrigger::CmpltToPattern || *t == WarningTrigger::GpuProgressError => {
                    state
                        .health_score
                        .record_event(HealthEventKind::PcieTransient);
                }
                WarningTrigger::XidError { .. } => {
                    state.health_score.record_event(HealthEventKind::XidError);
                }
                _ => {}
            }

            if let Some(new_level) = state.warning_machine.process_trigger(trigger) {
                // Update scheduler warning level
                state.scheduler.set_warning_level(new_level);

                // Get migration actions
                let pipelines_on_egpu: Vec<(String, u32)> = state
                    .scheduler
                    .pipelines_on_gpu(GpuTarget::Egpu)
                    .iter()
                    .map(|a| (a.name.clone(), a.priority))
                    .collect();

                let actions = state.warning_machine.migration_actions(&pipelines_on_egpu);

                // Check if recovery should be spawned
                let should_start_recovery = new_level >= WarningLevel::Orange && !state.recovery_active;
                if should_start_recovery {
                    state.recovery_active = true;
                }

                Some((new_level, actions, should_start_recovery))
            } else {
                None
            }
        };
        // Lock dropped here — db.log_event(), SSE, recovery happen outside lock

        if let Some((new_level, actions, should_start_recovery)) = level_change {
            let severity = match new_level {
                WarningLevel::Green => Severity::Info,
                WarningLevel::Yellow => Severity::Warning,
                WarningLevel::Orange => Severity::Error,
                WarningLevel::Red => Severity::Critical,
            };

            if let Err(e) = self
                .db
                .log_event(
                    "warning.level_change",
                    severity,
                    &format!("Warnstufe: {} (Auslöser: {})", new_level, trigger_str),
                    Some(serde_json::json!({
                        "level": format!("{new_level}"),
                        "trigger": trigger_str,
                    })),
                )
                .await
            {
                error!("Event-Logging fehlgeschlagen: {e}");
            }

            // Broadcast SSE warning level event (Gap 2)
            if let Some(ref sse) = self.sse {
                sse.send(BroadcastEvent::WarningLevel(serde_json::json!({
                    "level": format!("{new_level}"),
                    "trigger": trigger_str,
                })));
            }

            // FIX 6: ntfy-Benachrichtigung bei Orange oder Red
            if new_level >= WarningLevel::Orange {
                send_ntfy_notification(
                    &self.config,
                    &format!("eGPU Warnstufe: {new_level}"),
                    &format!("Ausloeser: {trigger_str}"),
                    if new_level >= WarningLevel::Red {
                        "urgent"
                    } else {
                        "high"
                    },
                );
            }

            for action in &actions {
                info!(
                    "Migration-Aktion: {} — {:?} (Prio {})",
                    action.pipeline_name, action.action, action.priority
                );
            }

            // Gap 3: Try pressure reduction before full recovery at Orange
            if should_start_recovery {
                let config_clone = Arc::clone(&self.config);
                let db_clone = self.db.clone();
                let sse_clone = self.sse.clone();
                let state_clone = Arc::clone(&self.state);

                let affected = get_egpu_pipelines(&config_clone);

                tokio::spawn(async move {
                    // Try pressure reduction first
                    let reduced = Self::try_pressure_reduction(
                        Arc::clone(&config_clone),
                        Arc::clone(&state_clone),
                        sse_clone.clone(),
                    )
                    .await;

                    if reduced {
                        info!("Druckreduktion erfolgreich — Recovery vermieden");
                        let mut st = state_clone.lock().await;
                        st.recovery_active = false;
                        // FIX 10: Aktive Trigger zuruecksetzen nach erfolgreicher Druckreduktion
                        st.warning_machine.process_trigger(WarningTrigger::AllClear);
                    } else {
                        info!("Druckreduktion nicht ausreichend — starte volle Recovery");
                        Self::run_recovery_task(
                            config_clone,
                            db_clone,
                            sse_clone,
                            state_clone,
                            affected,
                        )
                        .await;
                    }
                });
            }
        }
    }

    /// GPU polling loop: periodically queries nvidia-smi and Ollama.
    /// Includes idle model auto-unloading, thermal protection, proactive
    /// freeze prevention (thermal gradient, P-state tracking, nvidia-smi
    /// latency monitoring), adaptive poll rate, and health score updates.
    async fn gpu_polling_loop(
        state: Arc<Mutex<MonitorState>>,
        config: Arc<Config>,
        cancel: CancellationToken,
        sse: Option<SseBroadcaster>,
        trigger_tx: mpsc::Sender<WarningTrigger>,
        db: EventDb,
        metrics: Option<Arc<crate::metrics::DaemonMetrics>>,
    ) {
        let gpu_monitor = GpuMonitorBackend::new(config.gpu.nvidia_smi_timeout_seconds);
        let mut consecutive_timeouts: u32 = 0;
        let mut last_telemetry_log = std::time::Instant::now();

        // Idle model tracking
        let mut last_gpu_activity: Option<std::time::Instant> = None;
        let mut models_loaded = false;

        // FIX 24: Power-Draw-Anomalie-Erkennung
        let mut power_baseline: Option<f64> = None;

        // SM Clock Variance tracking (exponential moving average)
        let mut clock_baseline: Option<f64> = None;
        // Power instability tracking (10s window ~ 2 samples at 5s polling)
        let mut power_history: VecDeque<f64> = VecDeque::with_capacity(10);

        // Proactive monitoring state
        let mut smi_response_times: VecDeque<u128> =
            VecDeque::with_capacity(config.gpu.nvidia_smi_response_avg_window as usize);
        let mut pstate_p4_since: Option<std::time::Instant> = None;
        // Thermischer Gradient: Ring-Buffer über 60s (12 Samples bei 5s Polling)
        // um Einzel-Sprünge beim Laststart zu glätten
        let mut temp_history: VecDeque<(u32, std::time::Instant)> = VecDeque::with_capacity(12);

        // Build Ollama control for auto-unloading (Legacy single-instance)
        let ollama_ctl = config.ollama.as_ref().and_then(|cfg| {
            if cfg.enabled {
                Some(crate::ollama::HttpOllamaControl::new(&cfg.host))
            } else {
                None
            }
        });

        // OllamaFleet für Multi-Instanz-Modus
        let mut ollama_fleet = {
            let instances = config.resolve_ollama_instances();
            if instances.is_empty() {
                None
            } else {
                Some(crate::ollama::OllamaFleet::new(
                    instances,
                    &config.gpu.egpu_pci_address,
                ))
            }
        };
        // Per-Instance Idle-Tracking
        let mut fleet_idle_trackers: HashMap<String, Option<std::time::Instant>> = HashMap::new();

        info!(
            "GPU-Poller gestartet (Intervall: {}s)",
            config.gpu.poll_interval_seconds
        );

        loop {
            // Compute adaptive poll interval based on current warning level
            let poll_interval = {
                let st = state.lock().await;
                let level = st.warning_machine.current_level();
                match level {
                    WarningLevel::Green => {
                        std::time::Duration::from_secs(config.gpu.poll_interval_seconds)
                    }
                    WarningLevel::Yellow => {
                        std::time::Duration::from_secs(config.gpu.fast_poll_interval_seconds)
                    }
                    WarningLevel::Orange | WarningLevel::Red => {
                        std::time::Duration::from_millis(config.gpu.emergency_poll_interval_ms)
                    }
                }
            };

            tokio::select! {
                _ = cancel.cancelled() => {
                    info!("GPU-Poller beendet");
                    return;
                }
                _ = tokio::time::sleep(poll_interval) => {
                    // Measure nvidia-smi response time
                    let query_start = std::time::Instant::now();
                    match gpu_monitor.query_all().await {
                        Ok(mut gpus) => {
                            let response_ms = query_start.elapsed().as_millis();
                            consecutive_timeouts = 0;

                            // Prometheus: nvidia query duration histogram
                            if let Some(ref m) = metrics {
                                m.nvidia_query_duration_ms.observe(response_ms as f64);
                            }

                            // Track nvidia-smi response time
                            let window = config.gpu.nvidia_smi_response_avg_window as usize;
                            if smi_response_times.len() >= window {
                                smi_response_times.pop_front();
                            }
                            smi_response_times.push_back(response_ms);

                            // M2: Collect health events locally, apply in single lock
                            let mut pending_health_events: Vec<HealthEventKind> = Vec::new();
                            let mut pending_triggers: Vec<WarningTrigger> = Vec::new();

                            // Check for slow single response -> health score penalty
                            if response_ms > 1000 {
                                pending_health_events.push(HealthEventKind::NvidiaSmiSlow);
                            }

                            // Check average response time
                            if smi_response_times.len() >= window {
                                let avg: u128 = smi_response_times.iter().sum::<u128>()
                                    / smi_response_times.len() as u128;
                                if avg > config.gpu.nvidia_smi_slow_threshold_ms as u128 {
                                    warn!(
                                        "nvidia-smi Durchschnitts-Antwortzeit {}ms > {}ms",
                                        avg, config.gpu.nvidia_smi_slow_threshold_ms
                                    );
                                    pending_triggers.push(WarningTrigger::NvidiaSmiSlow);
                                }
                            }

                            // Set gpu_type based on config
                            for gpu in &mut gpus {
                                if gpu.pci_address == config.gpu.egpu_pci_address {
                                    gpu.gpu_type = GpuType::Egpu;
                                } else if gpu.pci_address == config.gpu.internal_pci_address {
                                    gpu.gpu_type = GpuType::Internal;
                                }
                            }

                            // M5: Track eGPU availability for scheduler
                            let egpu_visible = gpus.iter().any(|g| g.pci_address == config.gpu.egpu_pci_address);

                            // eGPU-specific proactive checks (no lock needed — local analysis)
                            if let Some(egpu_status) = gpus.iter().find(|g| g.pci_address == config.gpu.egpu_pci_address) {
                                // --- P-State throttling detection ---
                                let pstate_num = egpu_status.pstate
                                    .trim_start_matches('P')
                                    .parse::<u32>()
                                    .unwrap_or(0);

                                let gpu_is_active = egpu_status.utilization_gpu_percent > 5
                                    || (egpu_status.memory_used_mb > 500 && egpu_status.utilization_gpu_percent > 0);
                                let is_throttle_state = pstate_num >= config.gpu.pstate_throttle_threshold
                                    && pstate_num < 8;

                                if is_throttle_state && gpu_is_active {
                                    match pstate_p4_since {
                                        Some(since) => {
                                            let sustained = since.elapsed().as_secs();
                                            if sustained >= config.gpu.pstate_throttle_sustained_seconds {
                                                warn!(
                                                    "P-State P{} sustained {}s bei aktiver GPU ({}%) — Anomalie",
                                                    pstate_num, sustained,
                                                    egpu_status.utilization_gpu_percent
                                                );
                                                pending_triggers.push(WarningTrigger::PstateThrottle);
                                                pending_health_events.push(HealthEventKind::PstateAnomaly);
                                                pstate_p4_since = Some(std::time::Instant::now());
                                            }
                                        }
                                        None => {
                                            pstate_p4_since = Some(std::time::Instant::now());
                                        }
                                    }
                                } else {
                                    pstate_p4_since = None;
                                }

                                // --- Thermal gradient detection ---
                                let current_temp = egpu_status.temperature_c;
                                let now_instant = std::time::Instant::now();
                                temp_history.push_back((current_temp, now_instant));

                                while temp_history.len() > 1
                                    && temp_history.front().unwrap().1.elapsed().as_secs() > 60
                                {
                                    temp_history.pop_front();
                                }

                                if temp_history.len() >= 6 {
                                    let (oldest_temp, oldest_time) = temp_history.front().unwrap();
                                    let elapsed_min = oldest_time.elapsed().as_secs_f64() / 60.0;
                                    if elapsed_min > 0.1 {
                                        let gradient = if current_temp > *oldest_temp {
                                            (current_temp - oldest_temp) as f64 / elapsed_min
                                        } else {
                                            0.0
                                        };

                                        let temp_threshold = 76u32;
                                        if gradient > config.gpu.thermal_gradient_warning_c_per_min
                                            && current_temp >= temp_threshold
                                        {
                                            warn!(
                                                "Thermischer Gradient {:.1}°C/min > {:.1}°C/min (Temp: {}°C, Fenster: {}s)",
                                                gradient,
                                                config.gpu.thermal_gradient_warning_c_per_min,
                                                current_temp,
                                                oldest_time.elapsed().as_secs(),
                                            );
                                            pending_health_events.push(HealthEventKind::TemperatureSpike);
                                        }
                                    }
                                }

                                // --- Absolute thermal thresholds ---
                                if current_temp >= config.gpu.thermal_critical_temp_c {
                                    pending_triggers.push(WarningTrigger::ThermalCritical);
                                    pending_health_events.push(HealthEventKind::TemperatureSpike);
                                } else if current_temp >= config.gpu.thermal_throttle_temp_c {
                                    pending_triggers.push(WarningTrigger::ThermalThrottle);
                                }

                                // FIX 24: Power-Draw-Anomalie-Erkennung
                                let power_w = egpu_status.power_draw_w;
                                let tdp_w = 300.0_f64;

                                power_baseline = Some(match power_baseline {
                                    Some(baseline) => baseline * 0.95 + power_w * 0.05,
                                    None => power_w,
                                });

                                if power_w < 5.0 && egpu_status.utilization_gpu_percent > 10 {
                                    warn!(
                                        "Power-Draw-Anomalie: {:.1}W bei {}% Auslastung — moeglicherweise PCIe-Link-Problem",
                                        power_w, egpu_status.utilization_gpu_percent
                                    );
                                    pending_health_events.push(HealthEventKind::PcieTransient);
                                }

                                if power_w > tdp_w * 1.1 {
                                    warn!(
                                        "Power-Draw {:.1}W ueberschreitet TDP*1.1 ({:.1}W) — thermisches Risiko",
                                        power_w, tdp_w * 1.1
                                    );
                                    pending_health_events.push(HealthEventKind::TemperatureSpike);
                                }

                                // Track GPU activity for idle detection
                                if egpu_status.utilization_gpu_percent > 0 {
                                    last_gpu_activity = Some(std::time::Instant::now());
                                }
                            }

                            // SM Clock Variance + Power Instability detection (no lock)
                            if let Some(egpu) = gpus.iter().find(|g| g.pci_address == config.gpu.egpu_pci_address) {
                                let clock = egpu.clock_graphics_mhz as f64;
                                clock_baseline = Some(match clock_baseline {
                                    Some(baseline) => baseline * 0.95 + clock * 0.05,
                                    None => clock,
                                });
                                if let Some(baseline) = clock_baseline {
                                    if clock < baseline * 0.8 && egpu.utilization_gpu_percent > 50 {
                                        warn!(
                                            "SM-Clock-Varianz: {} MHz < 80% von Baseline {:.0} MHz bei {}% Auslastung",
                                            egpu.clock_graphics_mhz, baseline, egpu.utilization_gpu_percent
                                        );
                                        pending_health_events.push(HealthEventKind::SmClockVariance);
                                    }
                                }

                                let power = egpu.power_draw_w;
                                if power_history.len() >= 10 {
                                    power_history.pop_front();
                                }
                                power_history.push_back(power);
                                if power_history.len() >= 3 {
                                    let mean = power_history.iter().sum::<f64>() / power_history.len() as f64;
                                    let variance = power_history.iter()
                                        .map(|p| (p - mean).powi(2))
                                        .sum::<f64>() / power_history.len() as f64;
                                    let stddev = variance.sqrt();
                                    if mean > 50.0 && stddev > mean * 0.3 {
                                        warn!(
                                            "Power-Instabilitaet: Stddev {:.1}W bei Mean {:.1}W (>30%)",
                                            stddev, mean
                                        );
                                        pending_health_events.push(HealthEventKind::PowerInstability);
                                    }
                                }
                            }

                            // Power budget check (no lock)
                            if config.gpu.max_combined_power_w > 0.0 {
                                let combined_power: f64 = gpus.iter().map(|g| g.power_draw_w).sum();
                                if combined_power > config.gpu.max_combined_power_w * 0.9 {
                                    warn!(
                                        "Kombinierte GPU-Last {:.0}W > 90% von {:.0}W Budget",
                                        combined_power, config.gpu.max_combined_power_w
                                    );
                                }
                            }

                            // Query compute processes outside lock (sync NVML call)
                            let mut internal_display_reserves: Vec<(GpuTarget, u64)> = Vec::new();
                            for gpu in &gpus {
                                let target = if gpu.pci_address == config.gpu.egpu_pci_address {
                                    GpuTarget::Egpu
                                } else {
                                    GpuTarget::Internal
                                };
                                if target == GpuTarget::Internal {
                                    if let Ok(procs) = gpu_monitor.query_compute_processes(&gpu.pci_address) {
                                        let total_compute: u64 = procs.iter().map(|p| p.used_mb).sum();
                                        const DISPLAY_SAFETY_HEADROOM_MB: u64 = 512;
                                        let display_vram = gpu.memory_used_mb.saturating_sub(total_compute);
                                        let effective_reserve = (display_vram + DISPLAY_SAFETY_HEADROOM_MB).max(config.gpu.display_vram_reserve_mb);
                                        internal_display_reserves.push((target, effective_reserve));
                                    }
                                }
                            }

                            // Idle tracking and thermal protection for eGPU (Ollama)
                            // Fleet-Modus: Thermal-Unload pro Instanz
                            if let Some(ref fleet) = ollama_fleet {
                                let egpu = gpus.iter().find(|g| g.pci_address == config.gpu.egpu_pci_address);
                                if let Some(egpu_status) = egpu {
                                    for inst in fleet.all_instances() {
                                        if inst.gpu_target == crate::scheduler::GpuTarget::Egpu
                                            && inst.config.thermal_unload_temp_c > 0
                                            && egpu_status.temperature_c >= inst.config.thermal_unload_temp_c
                                            && inst.available
                                        {
                                            warn!(
                                                "GPU-Temperatur {}°C >= {}°C — entlade Modelle auf '{}'",
                                                egpu_status.temperature_c,
                                                inst.config.thermal_unload_temp_c,
                                                inst.config.name,
                                            );
                                            Self::unload_all_ollama_models(&inst.control, &state).await;
                                        }
                                    }
                                }
                            } else if let Some(ref ollama_cfg) = config.ollama {
                                let egpu = gpus.iter().find(|g| g.pci_address == config.gpu.egpu_pci_address);
                                if let Some(egpu_status) = egpu {
                                    if ollama_cfg.thermal_unload_temp_c > 0
                                        && egpu_status.temperature_c >= ollama_cfg.thermal_unload_temp_c
                                    {
                                        if let Some(ref ctl) = ollama_ctl {
                                            warn!(
                                                "GPU-Temperatur {}°C >= Schwellenwert {}°C — entlade Ollama-Modelle präventiv",
                                                egpu_status.temperature_c, ollama_cfg.thermal_unload_temp_c
                                            );
                                            Self::unload_all_ollama_models(ctl, &state).await;
                                            models_loaded = false;
                                        }
                                    }
                                }
                            }

                            // M2: Single bulk lock acquisition for all state updates
                            let (health_score_summary, health_trigger, telemetry_data) = {
                                let lock_start = std::time::Instant::now();
                                let mut st = state.lock().await;

                                // Apply all pending health events
                                for event in &pending_health_events {
                                    st.health_score.record_event(event.clone());
                                }

                                // M5: Update eGPU availability in scheduler
                                st.scheduler.set_egpu_available(egpu_visible);

                                // Update scheduler compute utilization + VRAM capacities
                                for gpu in &gpus {
                                    let target = if gpu.pci_address == config.gpu.egpu_pci_address {
                                        GpuTarget::Egpu
                                    } else {
                                        GpuTarget::Internal
                                    };
                                    st.scheduler.set_compute_utilization(target, gpu.utilization_gpu_percent);
                                    st.scheduler.update_total_vram(target, gpu.memory_total_mb);

                                    if target == GpuTarget::Internal {
                                        const DISPLAY_SAFETY_HEADROOM_MB: u64 = 512;
                                        let dynamic_reserve = gpu.memory_used_mb + DISPLAY_SAFETY_HEADROOM_MB;
                                        let effective_reserve = dynamic_reserve.max(config.gpu.display_vram_reserve_mb);
                                        st.scheduler.update_display_reserve(target, effective_reserve);
                                    }
                                }

                                // Apply compute-process-based display reserves
                                for (target, reserve) in &internal_display_reserves {
                                    st.scheduler.update_display_reserve(*target, *reserve);
                                }

                                st.gpu_status = gpus.clone();

                                // Prometheus metrics update
                                if let Some(ref m) = metrics {
                                    for gpu in &gpus {
                                        let label = if gpu.pci_address == config.gpu.egpu_pci_address { "egpu" } else { "internal" };
                                        m.update_gpu(label, gpu.temperature_c, gpu.utilization_gpu_percent,
                                            gpu.memory_used_mb, gpu.memory_total_mb, gpu.power_draw_w,
                                            gpu.clock_graphics_mhz, gpu.clock_memory_mhz);
                                    }
                                    m.scheduler_queue_length.set(st.scheduler.queue().len() as i64);
                                    m.active_leases_total.set(st.active_leases.len() as i64);
                                    m.recovery_active.set(if st.recovery_active { 1 } else { 0 });
                                    m.health_score.set(st.health_score.current_score());
                                    let wl = match st.warning_machine.current_level() {
                                        WarningLevel::Green => 0,
                                        WarningLevel::Yellow => 1,
                                        WarningLevel::Orange => 2,
                                        WarningLevel::Red => 3,
                                    };
                                    m.set_warning_level(wl);

                                    // Scheduler-Metriken pro GPU
                                    for (label, target) in [
                                        ("egpu", crate::scheduler::GpuTarget::Egpu),
                                        ("internal", crate::scheduler::GpuTarget::Internal),
                                    ] {
                                        let vram_used = st.scheduler.vram_used(target) as i64;
                                        let vram_available = st.scheduler.vram_available(target) as i64;
                                        m.update_scheduler(label, 0, vram_used, vram_available);
                                    }
                                }

                                // Health score tick
                                let health_trigger = st.health_score.tick();
                                let health_summary = st.health_score.summary();

                                // Telemetry data (collect under lock, log after drop)
                                let telemetry_data = if last_telemetry_log.elapsed() >= std::time::Duration::from_secs(30) {
                                    Some((st.health_score.current_score(), format!("{}", st.warning_machine.current_level())))
                                } else {
                                    None
                                };

                                let lock_hold_us = lock_start.elapsed().as_micros();
                                debug!("GPU-Poller lock_hold_duration_us={lock_hold_us}");

                                (health_summary, health_trigger, telemetry_data)
                            };
                            // Lock dropped here

                            // Fleet: GPU-Verfügbarkeit aktualisieren (außerhalb des Locks)
                            if let Some(ref mut fleet) = ollama_fleet {
                                fleet.set_gpu_available(
                                    crate::scheduler::GpuTarget::Egpu,
                                    egpu_visible,
                                );
                            }

                            // Send pending triggers (after lock drop)
                            for trigger in pending_triggers {
                                let _ = trigger_tx.send(trigger).await;
                            }

                            // Send health score trigger if needed
                            if let Some(trigger) = health_trigger {
                                let _ = trigger_tx.send(trigger).await;
                            }

                            // SSE broadcasts (after lock drop)
                            if let Some(ref sse) = sse {
                                sse.send(BroadcastEvent::HealthScore(health_score_summary));

                                let gpu_summary: Vec<serde_json::Value> = gpus
                                    .iter()
                                    .map(|g| {
                                        serde_json::json!({
                                            "pci_address": g.pci_address,
                                            "name": g.name,
                                            "temperature_c": g.temperature_c,
                                            "utilization_gpu_percent": g.utilization_gpu_percent,
                                            "memory_used_mb": g.memory_used_mb,
                                            "memory_total_mb": g.memory_total_mb,
                                        })
                                    })
                                    .collect();
                                sse.send(BroadcastEvent::GpuStatus(
                                    serde_json::json!({ "gpus": gpu_summary }),
                                ));
                            }

                            // Telemetry logging (after lock drop, M1)
                            if let Some((hs, wl)) = telemetry_data {
                                for gpu in &gpus {
                                    let gpu_type = if gpu.pci_address == config.gpu.egpu_pci_address {
                                        "egpu"
                                    } else {
                                        "internal"
                                    };
                                    if let Err(e) = db.log_gpu_telemetry(
                                        &gpu.pci_address,
                                        gpu_type,
                                        gpu.temperature_c,
                                        gpu.utilization_gpu_percent,
                                        gpu.memory_used_mb,
                                        gpu.memory_total_mb,
                                        gpu.power_draw_w,
                                        &gpu.pstate,
                                        gpu.fan_speed_percent,
                                        gpu.clock_graphics_mhz,
                                        Some(hs),
                                        Some(&wl),
                                    ).await {
                                        debug!("Telemetrie-Logging fehlgeschlagen: {e}");
                                    }
                                }
                                last_telemetry_log = std::time::Instant::now();
                            }
                        }
                        Err(e) => {
                            let response_ms = query_start.elapsed().as_millis();
                            warn!("nvidia-smi Abfrage fehlgeschlagen ({}ms): {e}", response_ms);
                            consecutive_timeouts += 1;

                            // Track slow/failed responses
                            let window = config.gpu.nvidia_smi_response_avg_window as usize;
                            if smi_response_times.len() >= window {
                                smi_response_times.pop_front();
                            }
                            smi_response_times.push_back(response_ms);

                            if consecutive_timeouts >= config.gpu.nvidia_smi_max_consecutive_timeouts {
                                warn!(
                                    "nvidia-smi {} aufeinanderfolgende Timeouts",
                                    consecutive_timeouts
                                );
                                let _ = trigger_tx.send(WarningTrigger::NvidiaSmiTimeout).await;
                            }

                            // Mark GPUs offline
                            let mut st = state.lock().await;
                            for gpu in &mut st.gpu_status {
                                if gpu.pci_address == config.gpu.egpu_pci_address {
                                    gpu.status = egpu_manager_common::gpu::GpuOnlineStatus::Timeout;
                                }
                            }
                            drop(st);

                            // FIX 26: Exponentieller Backoff bei aufeinanderfolgenden Fehlern
                            let backoff_secs = config.gpu.poll_interval_seconds
                                * (1u64 << consecutive_timeouts.min(5));
                            debug!(
                                "nvidia-smi Backoff: {}s (Fehler: {})",
                                backoff_secs, consecutive_timeouts
                            );
                            tokio::time::sleep(std::time::Duration::from_secs(backoff_secs)).await;
                        }
                    }

                    // Query PCIe throughput for eGPU
                    match gpu_monitor
                        .query_pcie_throughput(&config.gpu.egpu_pci_address)
                        .await
                    {
                        Ok(throughput) => {
                            let mut st = state.lock().await;
                            st.pcie_throughput
                                .insert(config.gpu.egpu_pci_address.clone(), throughput);
                        }
                        Err(_) => {
                            // PCIe throughput query failure is non-critical
                        }
                    }

                    // Query Ollama models — Fleet-Modus hat Vorrang
                    if let Some(ref fleet) = ollama_fleet {
                        let all_models = fleet.query_all_models().await;
                        let mut combined_models = Vec::new();

                        {
                            let mut st = state.lock().await;
                            st.ollama_models_by_instance = all_models.clone();

                            // Modelle im Scheduler registrieren
                            // Zuerst alle alten Model-Assignments entfernen
                            let old_keys: Vec<_> = st.scheduler.loaded_models().keys().cloned().collect();
                            for key in old_keys {
                                st.scheduler.unregister_model(&key.0, &key.1);
                            }

                            for (inst_name, models) in &all_models {
                                for model in models {
                                    combined_models.push(model.clone());
                                    // GPU-Target aus der Fleet-Instanz ableiten
                                    if let Some(inst) = fleet.instance_by_name(inst_name) {
                                        let vram_mb = model.size_vram_bytes / 1024 / 1024;
                                        st.scheduler.register_model(
                                            crate::scheduler::ModelAssignment {
                                                model_name: model.name.clone(),
                                                instance_name: inst_name.clone(),
                                                target: inst.gpu_target,
                                                vram_mb,
                                                priority: inst.config.priority,
                                                last_used: std::time::Instant::now(),
                                                workload_type: inst.config.workload_types
                                                    .first()
                                                    .cloned()
                                                    .unwrap_or_default(),
                                            },
                                        );
                                    }
                                }

                                // Per-Instance Idle-Tracking
                                let has_models = all_models.get(inst_name).map(|m| !m.is_empty()).unwrap_or(false);
                                if has_models {
                                    fleet_idle_trackers
                                        .entry(inst_name.clone())
                                        .or_insert_with(|| Some(std::time::Instant::now()));
                                }
                            }

                            st.ollama_models = combined_models;
                        }
                    } else if let Some(ref ollama_cfg) = config.ollama
                        && ollama_cfg.enabled
                    {
                        // Legacy: Einzelinstanz-Polling
                        match crate::nvidia::query_ollama_models(&ollama_cfg.host).await {
                            Ok(models) => {
                                let has_models = !models.is_empty();
                                {
                                    let mut st = state.lock().await;
                                    st.ollama_models = models;
                                }

                                // Idle auto-unload
                                if has_models {
                                    if !models_loaded {
                                        models_loaded = true;
                                        if last_gpu_activity.is_none() {
                                            last_gpu_activity = Some(std::time::Instant::now());
                                        }
                                    }

                                    let idle_threshold = std::time::Duration::from_secs(
                                        ollama_cfg.auto_unload_idle_minutes * 60,
                                    );

                                    let idle_duration = last_gpu_activity
                                        .map(|t| t.elapsed())
                                        .unwrap_or(std::time::Duration::ZERO);

                                    if idle_duration >= idle_threshold {
                                        info!(
                                            "GPU idle seit {} Minuten (Schwelle: {} Min.) — entlade Ollama-Modelle",
                                            idle_duration.as_secs() / 60,
                                            ollama_cfg.auto_unload_idle_minutes
                                        );
                                        if let Some(ref ctl) = ollama_ctl {
                                            Self::unload_all_ollama_models(ctl, &state).await;
                                            models_loaded = false;
                                            last_gpu_activity = None;
                                        }
                                    } else {
                                        debug!(
                                            "Ollama-Modelle geladen, GPU idle seit {}s (Schwelle: {}s)",
                                            idle_duration.as_secs(),
                                            idle_threshold.as_secs()
                                        );
                                    }
                                } else {
                                    models_loaded = false;
                                }
                            }
                            Err(_) => {
                                // Ollama query failure is non-critical
                            }
                        }
                    }
                }
            }
        }
    }

    /// Unload all currently loaded Ollama models.
    async fn unload_all_ollama_models(
        ollama: &dyn OllamaControl,
        state: &Arc<Mutex<MonitorState>>,
    ) {
        match ollama.list_running_models().await {
            Ok(models) => {
                for model in &models {
                    info!("Auto-Unload: Entlade Ollama-Modell '{}'", model.name);
                    if let Err(e) = ollama.unload_model(&model.name).await {
                        warn!("Fehler beim Entladen von '{}': {}", model.name, e);
                    }
                }
                // Clear models from state
                let mut st = state.lock().await;
                st.ollama_models.clear();
            }
            Err(e) => {
                warn!("Ollama-Modelle nicht abfragbar für Auto-Unload: {}", e);
            }
        }
    }

    /// Try pressure reduction before full recovery.
    /// Sets scheduler to Drain, unloads Ollama models, waits, then checks
    /// if nvidia-smi responds quickly. Returns true if GPU recovered.
    async fn try_pressure_reduction(
        config: Arc<Config>,
        state: Arc<Mutex<MonitorState>>,
        sse: Option<SseBroadcaster>,
    ) -> bool {
        // FIX 20: Pruefen ob eGPU noch physisch vorhanden ist
        if !is_egpu_present(&config.gpu.egpu_pci_address) {
            warn!(
                "eGPU unter {} nicht erreichbar (Kabel getrennt?) — ueberspringe Druckreduktion",
                config.gpu.egpu_pci_address
            );
            return false;
        }

        info!("Starte Druckreduktion vor Recovery");

        if let Some(ref sse) = sse {
            sse.send(BroadcastEvent::RecoveryStage(serde_json::json!({
                "action": "pressure_reduction_started",
                "stage": "drain",
            })));
        }

        // Step 1: Set scheduler to Drain
        {
            let mut st = state.lock().await;
            st.scheduler.set_admission_state(AdmissionState::Drain);
        }

        // Step 2: Unload Ollama models if configured
        // Fleet-Modus: Alle eGPU-Instanzen entladen
        let instances = config.resolve_ollama_instances();
        if !instances.is_empty() {
            for inst in &instances {
                if inst.gpu_device == config.gpu.egpu_pci_address {
                    let ctl = crate::ollama::HttpOllamaControl::new(&inst.host);
                    info!("Druckreduktion: Entlade Modelle auf '{}'", inst.name);
                    Self::unload_all_ollama_models(&ctl, &state).await;
                }
            }
        } else if let Some(ref ollama_cfg) = config.ollama {
            if ollama_cfg.enabled {
                let ctl = crate::ollama::HttpOllamaControl::new(&ollama_cfg.host);
                Self::unload_all_ollama_models(&ctl, &state).await;
            }
        }

        // Step 3: Wait for pressure to reduce
        let wait_secs = config.gpu.pressure_reduction_wait_seconds;
        info!("Druckreduktion: Warte {}s", wait_secs);
        tokio::time::sleep(std::time::Duration::from_secs(wait_secs)).await;

        // Step 4: Check if GPU responds quickly (NVML or nvidia-smi fallback)
        let gpu_monitor = GpuMonitorBackend::new(config.gpu.nvidia_smi_timeout_seconds);
        let start = std::time::Instant::now();
        match gpu_monitor.query_all().await {
            Ok(_) => {
                let response_ms = start.elapsed().as_millis();
                let threshold = config.gpu.nvidia_smi_slow_threshold_ms as u128;

                if response_ms < threshold {
                    info!(
                        "Druckreduktion erfolgreich: GPU antwortet in {}ms (< {}ms)",
                        response_ms, threshold
                    );

                    // Restore admission to Open
                    {
                        let mut st = state.lock().await;
                        st.scheduler.set_admission_state(AdmissionState::Open);
                    }

                    if let Some(ref sse) = sse {
                        sse.send(BroadcastEvent::RecoveryStage(serde_json::json!({
                            "action": "pressure_reduction_success",
                            "response_ms": response_ms,
                        })));
                    }

                    return true;
                }

                warn!(
                    "Druckreduktion nicht ausreichend: nvidia-smi {}ms >= {}ms",
                    response_ms, threshold
                );
            }
            Err(e) => {
                warn!("Druckreduktion: nvidia-smi nicht erreichbar: {e}");
            }
        }

        if let Some(ref sse) = sse {
            sse.send(BroadcastEvent::RecoveryStage(serde_json::json!({
                "action": "pressure_reduction_failed",
            })));
        }

        false
    }

    /// Lease expiry loop: removes expired leases.
    async fn lease_expiry_loop(state: Arc<Mutex<MonitorState>>, cancel: CancellationToken) {
        let interval = std::time::Duration::from_secs(5);
        loop {
            tokio::select! {
                _ = cancel.cancelled() => {
                    info!("Lease-Expiry-Task beendet");
                    return;
                }
                _ = tokio::time::sleep(interval) => {
                    // Heartbeat timeout: 2x the expected interval (60s default)
                    const HEARTBEAT_TIMEOUT_SECS: i64 = 60;
                    let now = Utc::now();

                    // M4: Collect stale lease info under lock, log after drop
                    let expired_info: Vec<(String, &'static str, String, u64)> = {
                        let mut st = state.lock().await;
                        let stale_leases: Vec<String> = st
                            .active_leases
                            .iter()
                            .filter(|(_, lease)| {
                                let heartbeat_stale = (now - lease.last_heartbeat).num_seconds() > HEARTBEAT_TIMEOUT_SECS;
                                let expired = lease.expires_at <= now;
                                heartbeat_stale || expired
                            })
                            .map(|(id, _)| id.clone())
                            .collect();

                        let mut info_vec = Vec::new();
                        for id in stale_leases {
                            if let Some(lease) = st.active_leases.remove(&id) {
                                if lease.target_kind != LeaseTargetKind::Remote {
                                    st.scheduler.release_lease(&id);
                                }
                                let reason = if lease.expires_at <= now {
                                    "abgelaufen"
                                } else {
                                    "Heartbeat-Timeout"
                                };
                                info_vec.push((id, reason, lease.pipeline, lease.vram_mb));
                            }
                        }
                        info_vec
                    };
                    // Lock dropped here — log outside

                    for (id, reason, pipeline, vram_mb) in &expired_info {
                        info!(
                            "Lease {} {} (Pipeline: {}, VRAM: {} MB)",
                            id, reason, pipeline, vram_mb
                        );
                    }
                }
            }
        }
    }

    /// Run recovery in a background task (Gap 3).
    async fn run_recovery_task(
        config: Arc<Config>,
        db: EventDb,
        sse: Option<SseBroadcaster>,
        state: Arc<Mutex<MonitorState>>,
        affected_pipelines: Vec<String>,
    ) {
        // FIX 20: Pruefen ob eGPU noch physisch vorhanden ist vor Recovery
        if !is_egpu_present(&config.gpu.egpu_pci_address) {
            warn!(
                "eGPU unter {} nicht erreichbar (Kabel getrennt?) — ueberspringe Recovery",
                config.gpu.egpu_pci_address
            );
            let mut st = state.lock().await;
            st.recovery_active = false;
            return;
        }

        info!(
            "Recovery wird gestartet fuer {} Pipeline(s)",
            affected_pipelines.len()
        );

        if let Some(ref sse) = sse {
            sse.send(BroadcastEvent::RecoveryStage(serde_json::json!({
                "action": "recovery_started",
                "stage": "stage0_quiesce",
                "affected_pipelines": affected_pipelines,
            })));
        }

        let docker_ctl = DockerComposeControl::new(
            config.docker.container_restart_timeout_seconds,
            config.docker.container_stop_timeout_seconds,
        );

        let mut rsm = RecoveryStateMachine::new(db.clone(), config.recovery.reset_cooldown_seconds);

        if let Err(e) = rsm.start_recovery(affected_pipelines).await {
            error!("Recovery-Start fehlgeschlagen: {e}");
            let mut st = state.lock().await;
            st.recovery_active = false;
            return;
        }

        match rsm.run_recovery(&config, &docker_ctl, None, None).await {
            Ok(()) => {
                info!("Recovery abgeschlossen");
                if let Some(ref sse) = sse {
                    sse.send(BroadcastEvent::RecoveryStage(serde_json::json!({
                        "action": "recovery_completed",
                        "stage": "idle",
                    })));
                }
            }
            Err(e) => {
                error!("Recovery fehlgeschlagen: {e}");
                if let Some(ref sse) = sse {
                    sse.send(BroadcastEvent::RecoveryStage(serde_json::json!({
                        "action": "recovery_failed",
                        "error": e.to_string(),
                    })));
                }
            }
        }

        let mut st = state.lock().await;
        st.recovery_active = false;
    }

    async fn retention_loop(db: EventDb, config: Arc<Config>, cancel: CancellationToken) {
        let retention_days = config.database.retention_days;
        let aggregate_days = config.database.aggregate_after_days;
        let interval_hours = config.database.retention_check_interval_hours;
        let max_db_size_mb = config.database.max_db_size_mb;
        let interval = std::time::Duration::from_secs(u64::from(interval_hours) * 3600);

        loop {
            tokio::select! {
                _ = cancel.cancelled() => {
                    info!("Retention-Task beendet");
                    return;
                }
                _ = tokio::time::sleep(interval) => {
                    if let Err(e) = db.apply_retention(retention_days).await {
                        error!("Retention fehlgeschlagen: {e}");
                    }
                    if let Err(e) = db.aggregate_monitoring_events(aggregate_days).await {
                        error!("Aggregation fehlgeschlagen: {e}");
                    }
                    // Telemetrie-Daten aufräumen (gleiche Retention wie Events)
                    match db.clean_telemetry(retention_days).await {
                        Ok(n) if n > 0 => info!("Telemetrie: {n} alte Einträge gelöscht"),
                        Err(e) => error!("Telemetrie-Cleanup fehlgeschlagen: {e}"),
                        _ => {}
                    }

                    // FIX 19: Datenbankgroesse pruefen
                    if let Some(size_mb) = db.check_db_size_mb() {
                        if size_mb > u64::from(max_db_size_mb) {
                            warn!(
                                "Datenbankgroesse {}MB ueberschreitet Limit {}MB",
                                size_mb, max_db_size_mb
                            );
                        }
                    }
                }
            }
        }
    }

    async fn stepdown_loop(
        state: Arc<Mutex<MonitorState>>,
        db: EventDb,
        cancel: CancellationToken,
        sse: Option<SseBroadcaster>,
    ) {
        // Check for step-down every 10 seconds
        let interval = std::time::Duration::from_secs(10);

        loop {
            tokio::select! {
                _ = cancel.cancelled() => {
                    info!("Stepdown-Task beendet");
                    return;
                }
                _ = tokio::time::sleep(interval) => {
                    // FIX 21: sd_notify Watchdog-Ping an systemd
                    if let Ok(socket_path) = std::env::var("NOTIFY_SOCKET") {
                        let _ = std::os::unix::net::UnixDatagram::unbound()
                            .and_then(|s| s.send_to(b"WATCHDOG=1", &socket_path));
                    }

                    // M1: Collect data under lock, drop before async I/O
                    let step_down_result = {
                        let mut state = state.lock().await;
                        if let Some(new_level) = state.warning_machine.try_step_down() {
                            state.scheduler.set_warning_level(new_level);
                            Some(new_level)
                        } else {
                            None
                        }
                    };
                    // Lock dropped here

                    if let Some(new_level) = step_down_result {
                        let severity = match new_level {
                            WarningLevel::Green => Severity::Info,
                            WarningLevel::Yellow => Severity::Warning,
                            WarningLevel::Orange => Severity::Error,
                            WarningLevel::Red => Severity::Critical,
                        };

                        if let Err(e) = db.log_event(
                            "warning.level_change",
                            severity,
                            &format!("Warnstufe gesenkt (Cooldown): {new_level}"),
                            Some(serde_json::json!({
                                "level": format!("{new_level}"),
                                "trigger": "cooldown_stepdown",
                            })),
                        ).await {
                            error!("Event-Logging fehlgeschlagen: {e}");
                        }

                        // Broadcast SSE event
                        if let Some(ref sse) = sse {
                            sse.send(BroadcastEvent::WarningLevel(serde_json::json!({
                                "level": format!("{new_level}"),
                                "trigger": "cooldown_stepdown",
                            })));
                        }
                    }
                }
            }
        }
    }

    /// FIX 7: CUDA Watchdog Loop — prueft periodisch ob CUDA-Operationen antworten.
    async fn cuda_watchdog_loop(
        config: Arc<Config>,
        trigger_tx: mpsc::Sender<WarningTrigger>,
        cancel: CancellationToken,
    ) {
        let binary = &config.gpu.cuda_watchdog_binary;
        let interval_ms = config.gpu.cuda_watchdog_interval_ms;
        let timeout_ms = config.gpu.cuda_watchdog_timeout_ms;

        // Pruefen ob Binary existiert
        if !std::path::Path::new(binary).exists() {
            warn!(
                "CUDA-Watchdog-Binary '{}' nicht gefunden — Watchdog deaktiviert",
                binary
            );
            return;
        }

        info!(
            "CUDA-Watchdog gestartet (Intervall: {}ms, Timeout: {}ms, Binary: {})",
            interval_ms, timeout_ms, binary
        );

        let interval = std::time::Duration::from_millis(interval_ms);
        let timeout = std::time::Duration::from_millis(timeout_ms);

        loop {
            tokio::select! {
                _ = cancel.cancelled() => {
                    info!("CUDA-Watchdog beendet");
                    return;
                }
                _ = tokio::time::sleep(interval) => {
                    let result = tokio::time::timeout(
                        timeout,
                        tokio::process::Command::new(binary)
                            .output(),
                    )
                    .await;

                    match result {
                        Ok(Ok(output)) => {
                            if !output.status.success() {
                                warn!(
                                    "CUDA-Watchdog: Binary '{}' mit Exit-Code {} beendet",
                                    binary,
                                    output.status.code().unwrap_or(-1)
                                );
                                let _ = trigger_tx.send(WarningTrigger::CudaWatchdogTimeout).await;
                            }
                        }
                        Ok(Err(e)) => {
                            warn!("CUDA-Watchdog: Fehler beim Ausfuehren von '{}': {}", binary, e);
                            let _ = trigger_tx.send(WarningTrigger::CudaWatchdogTimeout).await;
                        }
                        Err(_) => {
                            // Timeout
                            warn!(
                                "CUDA-Watchdog: Timeout ({}ms) bei '{}'",
                                timeout_ms, binary
                            );
                            let _ = trigger_tx.send(WarningTrigger::CudaWatchdogTimeout).await;
                        }
                    }
                }
            }
        }
    }
}

/// FIX 20: Prueft ob die eGPU noch physisch angeschlossen ist (PCI-Vendor-Datei vorhanden).
fn is_egpu_present(pci_address: &str) -> bool {
    std::path::Path::new(&format!("/sys/bus/pci/devices/{pci_address}/vendor")).exists()
}

fn find_pipeline_config<'a>(config: &'a Config, pipeline: &str) -> Option<&'a PipelineConfig> {
    config.pipeline.iter().find(|p| p.container == pipeline)
}

fn remote_gpu_config<'a>(config: &'a Config, name: &str) -> Option<&'a RemoteGpuConfig> {
    config.remote_gpu.iter().find(|r| r.name == name)
}

fn local_target_kind(target: GpuTarget) -> LeaseTargetKind {
    match target {
        GpuTarget::Egpu => LeaseTargetKind::Egpu,
        GpuTarget::Internal => LeaseTargetKind::Internal,
    }
}

fn local_available_vram(st: &MonitorState, target: GpuTarget) -> u64 {
    match target {
        GpuTarget::Egpu if !st.scheduler.egpu_available() => 0,
        _ => st.scheduler.vram_available(target),
    }
}

/// Check whether the Thunderbolt link to the eGPU is healthy enough for
/// workload placement. Returns false if the GPU is offline, sensors are
/// unreadable (temperature == 0), or the health score is below the warning
/// threshold.
fn thunderbolt_link_healthy(state: &MonitorState, egpu_pci: &str) -> bool {
    use egpu_manager_common::gpu::GpuOnlineStatus;

    // If no GPU status has been polled yet, assume healthy (don't block
    // initial operation before the first poll cycle completes).
    if state.gpu_status.is_empty() {
        return true;
    }

    if let Some(gpu) = state.gpu_status.iter().find(|g| g.pci_address == egpu_pci) {
        if gpu.status != GpuOnlineStatus::Online {
            return false;
        }
        if gpu.temperature_c == 0 {
            return false; // Sensor unreadable = link problem
        }
    } else {
        return false; // GPU not found in status despite other GPUs being present
    }

    // Check health score isn't critical
    if state.health_score.current_score() < state.health_score.warning_threshold() {
        return false;
    }

    true
}

fn remote_reserved_vram(st: &MonitorState, remote_name: &str) -> u64 {
    st.active_leases
        .values()
        .filter(|lease| {
            lease.target_kind == LeaseTargetKind::Remote
                && lease.remote_gpu_name.as_deref() == Some(remote_name)
        })
        .map(|lease| lease.vram_mb)
        .sum()
}

fn workload_is_remote_capable(config: &Config, pipeline: &str, workload_type: &str) -> bool {
    let Some(pipeline_cfg) = find_pipeline_config(config, pipeline) else {
        return false;
    };

    pipeline_cfg
        .remote_capable
        .iter()
        .any(|w| w == workload_type)
        && !pipeline_cfg.cuda_only.iter().any(|w| w == workload_type)
}

fn pipeline_priority(config: &Config, pipeline: &str) -> u32 {
    find_pipeline_config(config, pipeline)
        .map(|p| p.gpu_priority)
        .unwrap_or(3)
}

fn find_best_remote_candidate(
    st: &MonitorState,
    config: &Config,
    workload_type: &str,
    vram_mb: u64,
) -> Option<RemoteLeaseCandidate> {
    let mut candidates: Vec<RemoteLeaseCandidate> = st
        .remote_gpus
        .iter()
        .filter(|gpu| gpu.status == "online")
        .filter_map(|gpu| {
            let cfg = remote_gpu_config(config, &gpu.name);
            let total_vram = if gpu.vram_mb > 0 {
                gpu.vram_mb
            } else {
                cfg.map(|r| r.vram_mb).unwrap_or(0)
            };
            let available_vram_mb = total_vram.saturating_sub(remote_reserved_vram(st, &gpu.name));
            if available_vram_mb < vram_mb {
                return None;
            }

            if let Some(limit) = cfg
                .and_then(|r| r.max_latency_ms.get(workload_type))
                .copied()
            {
                let measured = gpu.latency_ms?;
                if u64::from(measured) > limit {
                    return None;
                }
            }

            Some(RemoteLeaseCandidate {
                name: gpu.name.clone(),
                host: gpu.host.clone(),
                port_ollama: gpu.port_ollama,
                port_agent: gpu.port_agent,
                available_vram_mb,
                latency_ms: gpu.latency_ms,
                priority: cfg.map(|r| r.priority).unwrap_or(u32::MAX),
            })
        })
        .collect();

    candidates.sort_by(|a, b| {
        a.priority
            .cmp(&b.priority)
            .then_with(|| {
                a.latency_ms
                    .unwrap_or(u32::MAX)
                    .cmp(&b.latency_ms.unwrap_or(u32::MAX))
            })
            .then_with(|| b.available_vram_mb.cmp(&a.available_vram_mb))
    });

    candidates.into_iter().next()
}

fn select_lease_placement(
    st: &MonitorState,
    config: &Config,
    pipeline: &str,
    workload_type: &str,
    vram_mb: u64,
) -> Option<LeasePlacement> {
    let warning_level = st.warning_machine.current_level();
    let priority = pipeline_priority(config, pipeline);
    let egpu_available = local_available_vram(st, GpuTarget::Egpu);
    let internal_available = local_available_vram(st, GpuTarget::Internal);
    let remote_candidate = if workload_is_remote_capable(config, pipeline, workload_type) {
        find_best_remote_candidate(st, config, workload_type, vram_mb)
    } else {
        None
    };

    let tb_healthy = thunderbolt_link_healthy(st, &config.gpu.egpu_pci_address);
    let egpu_usable = tb_healthy && st.scheduler.egpu_allows_priority(priority) && egpu_available >= vram_mb;
    let internal_usable = internal_available >= vram_mb;

    if warning_level >= WarningLevel::Yellow {
        if let Some(remote) = remote_candidate {
            return Some(LeasePlacement::Remote(remote));
        }
        if internal_usable {
            return Some(LeasePlacement::Local {
                target: GpuTarget::Internal,
                assignment_source: "warning_fallback".to_string(),
            });
        }
        return None;
    }

    if egpu_usable {
        if let Some(remote) = remote_candidate
            && egpu_available.saturating_sub(vram_mb) < EGPU_PROTECTION_HEADROOM_MB
        {
            return Some(LeasePlacement::Remote(remote));
        }

        return Some(LeasePlacement::Local {
            target: GpuTarget::Egpu,
            assignment_source: "preferred".to_string(),
        });
    }

    if let Some(remote) = remote_candidate {
        return Some(LeasePlacement::Remote(remote));
    }

    if internal_usable {
        return Some(LeasePlacement::Local {
            target: GpuTarget::Internal,
            assignment_source: "fallback".to_string(),
        });
    }

    None
}

pub fn recommend_gpu_placement(
    st: &MonitorState,
    config: &Config,
    pipeline: Option<&str>,
    workload_type: Option<&str>,
    vram_mb: Option<u64>,
) -> GpuRecommendation {
    let egpu_vram_available_mb = local_available_vram(st, GpuTarget::Egpu);
    let internal_vram_available_mb = local_available_vram(st, GpuTarget::Internal);

    let workload_type = workload_type.unwrap_or("generic");
    let pipeline = pipeline.unwrap_or("");
    let requested_vram = vram_mb.unwrap_or(0);
    let remote_candidate = if pipeline.is_empty() {
        None
    } else if workload_is_remote_capable(config, pipeline, workload_type) {
        find_best_remote_candidate(st, config, workload_type, requested_vram)
    } else {
        None
    };

    if !pipeline.is_empty() {
        if let Some(placement) =
            select_lease_placement(st, config, pipeline, workload_type, requested_vram)
        {
            return match placement {
                LeasePlacement::Local {
                    target,
                    assignment_source,
                } => {
                    let gpu_device = match target {
                        GpuTarget::Egpu => config.gpu.egpu_pci_address.clone(),
                        GpuTarget::Internal => config.gpu.internal_pci_address.clone(),
                    };
                    let gpu_info = st.gpu_status.iter().find(|g| g.pci_address == gpu_device);
                    GpuRecommendation {
                        recommended_gpu: match target {
                            GpuTarget::Egpu => "egpu".to_string(),
                            GpuTarget::Internal => "internal".to_string(),
                        },
                        recommended_device: gpu_device,
                        assignment_source,
                        target_kind: local_target_kind(target),
                        gpu_uuid: gpu_info.map(|g| g.gpu_uuid.clone()),
                        nvidia_index: gpu_info.and_then(|g| g.nvidia_index),
                        nvidia_visible_devices: gpu_info.map(|g| g.gpu_uuid.clone()),
                        remote_gpu_name: None,
                        remote_host: None,
                        remote_ollama_url: None,
                        remote_agent_url: None,
                        egpu_vram_available_mb,
                        internal_vram_available_mb,
                        remote_vram_available_mb: remote_candidate
                            .as_ref()
                            .map(|remote| remote.available_vram_mb),
                    }
                }
                LeasePlacement::Remote(remote) => GpuRecommendation {
                    recommended_gpu: "remote".to_string(),
                    recommended_device: format!("remote://{}", remote.name),
                    assignment_source: "remote".to_string(),
                    target_kind: LeaseTargetKind::Remote,
                    gpu_uuid: None,
                    nvidia_index: None,
                    nvidia_visible_devices: None,
                    remote_gpu_name: Some(remote.name.clone()),
                    remote_host: Some(remote.host.clone()),
                    remote_ollama_url: Some(format!(
                        "http://{}:{}",
                        remote.host, remote.port_ollama
                    )),
                    remote_agent_url: Some(format!("http://{}:{}", remote.host, remote.port_agent)),
                    egpu_vram_available_mb,
                    internal_vram_available_mb,
                    remote_vram_available_mb: Some(remote.available_vram_mb),
                },
            };
        }
    }

    if st.scheduler.egpu_allows_priority(3) && egpu_vram_available_mb >= internal_vram_available_mb
    {
        let gpu_info = st
            .gpu_status
            .iter()
            .find(|g| g.pci_address == config.gpu.egpu_pci_address);
        GpuRecommendation {
            recommended_gpu: "egpu".to_string(),
            recommended_device: config.gpu.egpu_pci_address.clone(),
            assignment_source: "generic".to_string(),
            target_kind: LeaseTargetKind::Egpu,
            gpu_uuid: gpu_info.map(|g| g.gpu_uuid.clone()),
            nvidia_index: gpu_info.and_then(|g| g.nvidia_index),
            nvidia_visible_devices: gpu_info.map(|g| g.gpu_uuid.clone()),
            remote_gpu_name: None,
            remote_host: None,
            remote_ollama_url: None,
            remote_agent_url: None,
            egpu_vram_available_mb,
            internal_vram_available_mb,
            remote_vram_available_mb: remote_candidate
                .as_ref()
                .map(|remote| remote.available_vram_mb),
        }
    } else {
        let gpu_info = st
            .gpu_status
            .iter()
            .find(|g| g.pci_address == config.gpu.internal_pci_address);
        GpuRecommendation {
            recommended_gpu: "internal".to_string(),
            recommended_device: config.gpu.internal_pci_address.clone(),
            assignment_source: "generic".to_string(),
            target_kind: LeaseTargetKind::Internal,
            gpu_uuid: gpu_info.map(|g| g.gpu_uuid.clone()),
            nvidia_index: gpu_info.and_then(|g| g.nvidia_index),
            nvidia_visible_devices: gpu_info.map(|g| g.gpu_uuid.clone()),
            remote_gpu_name: None,
            remote_host: None,
            remote_ollama_url: None,
            remote_agent_url: None,
            egpu_vram_available_mb,
            internal_vram_available_mb,
            remote_vram_available_mb: remote_candidate
                .as_ref()
                .map(|remote| remote.available_vram_mb),
        }
    }
}

/// Acquire a GPU lease. Returns the lease on success or a queue position.
pub async fn acquire_gpu_lease(
    state: &Arc<Mutex<MonitorState>>,
    config: &Config,
    pipeline: String,
    workload_type: String,
    vram_mb: u64,
    duration_seconds: u64,
) -> Result<GpuLease, u32> {
    let mut st = state.lock().await;

    match select_lease_placement(&st, config, &pipeline, &workload_type, vram_mb) {
        Some(LeasePlacement::Local {
            target,
            assignment_source,
        }) => {
            let now = Utc::now();
            let lease_id = Uuid::new_v4().to_string();
            let gpu_device = match target {
                GpuTarget::Egpu => config.gpu.egpu_pci_address.clone(),
                GpuTarget::Internal => config.gpu.internal_pci_address.clone(),
            };

            // Echte GPU UUID + nvidia_index aus MonitorState.gpu_status holen
            let (gpu_uuid, nvidia_index) = st
                .gpu_status
                .iter()
                .find(|g| g.pci_address == gpu_device)
                .map(|g| (g.gpu_uuid.clone(), g.nvidia_index))
                .unwrap_or_default();

            let lease = GpuLease {
                lease_id: lease_id.clone(),
                pipeline,
                gpu_device,
                gpu_uuid: gpu_uuid.clone(),
                vram_mb,
                workload_type,
                acquired_at: now,
                expires_at: now + chrono::Duration::seconds(duration_seconds as i64),
                target_kind: local_target_kind(target),
                nvidia_index,
                nvidia_visible_devices: if gpu_uuid.is_empty() {
                    None
                } else {
                    Some(gpu_uuid)
                },
                assignment_source,
                remote_gpu_name: None,
                remote_host: None,
                remote_ollama_url: None,
                remote_agent_url: None,
                last_heartbeat: now,
            };

            st.scheduler
                .reserve_lease(lease_id.clone(), target, vram_mb);
            st.active_leases.insert(lease_id, lease.clone());
            Ok(lease)
        }
        Some(LeasePlacement::Remote(remote)) => {
            let now = Utc::now();
            let lease_id = Uuid::new_v4().to_string();

            let lease = GpuLease {
                lease_id: lease_id.clone(),
                pipeline,
                gpu_device: format!("remote://{}", remote.name),
                gpu_uuid: String::new(),
                vram_mb,
                workload_type,
                acquired_at: now,
                expires_at: now + chrono::Duration::seconds(duration_seconds as i64),
                target_kind: LeaseTargetKind::Remote,
                nvidia_index: None,
                nvidia_visible_devices: None,
                assignment_source: "remote".to_string(),
                remote_gpu_name: Some(remote.name.clone()),
                remote_host: Some(remote.host.clone()),
                remote_ollama_url: Some(format!("http://{}:{}", remote.host, remote.port_ollama)),
                remote_agent_url: Some(format!("http://{}:{}", remote.host, remote.port_agent)),
                last_heartbeat: now,
            };

            st.active_leases.insert(lease_id, lease.clone());
            Ok(lease)
        }
        None => {
            // Return queue position (number of active leases + 1)
            let queue_position = (st.active_leases.len() + 1) as u32;
            Err(queue_position)
        }
    }
}

/// Release a GPU lease.
pub fn release_gpu_lease(state: &mut MonitorState, lease_id: &str) -> bool {
    if let Some(lease) = state.active_leases.remove(lease_id) {
        if lease.target_kind != LeaseTargetKind::Remote {
            state.scheduler.release_lease(lease_id);
        }
        true
    } else {
        false
    }
}

/// Sendet eine ntfy-Benachrichtigung (fire-and-forget).
/// Wird nur ausgefuehrt wenn ntfy_url und ntfy_topic konfiguriert sind.
fn send_ntfy_notification(config: &Config, title: &str, message: &str, priority: &str) {
    let ntfy_url = &config.notifications.ntfy_url;
    let ntfy_topic = &config.notifications.ntfy_topic;

    if ntfy_url.is_empty() || ntfy_topic.is_empty() {
        return;
    }

    let url = format!("{}/{}", ntfy_url.trim_end_matches('/'), ntfy_topic);
    let title = title.to_string();
    let message = message.to_string();
    let priority = priority.to_string();

    // Fire-and-forget: ntfy-Fehler sind nicht kritisch
    tokio::spawn(async move {
        let client = match reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(5))
            .build()
        {
            Ok(c) => c,
            Err(e) => {
                warn!("ntfy HTTP-Client Fehler: {e}");
                return;
            }
        };

        let result = client
            .post(&url)
            .header("Title", title)
            .header("Priority", priority)
            .header("Tags", "warning,gpu")
            .body(message)
            .send()
            .await;

        match result {
            Ok(resp) if resp.status().is_success() => {
                debug!("ntfy-Benachrichtigung gesendet");
            }
            Ok(resp) => {
                warn!("ntfy-Fehler: HTTP {}", resp.status());
            }
            Err(e) => {
                warn!("ntfy nicht erreichbar: {e}");
            }
        }
    });
}
