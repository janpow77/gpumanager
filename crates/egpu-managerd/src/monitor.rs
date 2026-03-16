use std::collections::{HashMap, VecDeque};
use std::sync::Arc;

use chrono::{DateTime, Utc};
use egpu_manager_common::config::Config;
use egpu_manager_common::gpu::{GpuStatus, GpuType, OllamaModel, PcieThroughput, WarningLevel};
use egpu_manager_common::hal::{AerMonitor, OllamaControl, PcieLinkMonitor};
use serde::{Deserialize, Serialize};
use tokio::sync::{mpsc, Mutex};
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, warn};
use uuid::Uuid;

use crate::aer::AerWatcher;
use crate::db::{EventDb, Severity};
use crate::docker::DockerComposeControl;
use crate::health_score::{HealthEventKind, LinkHealthScore};
use crate::kmsg::KmsgMonitor;
use crate::link_health::LinkHealthWatcher;
use crate::nvidia::NvidiaSmiMonitor;
use crate::recovery::{get_egpu_pipelines, RecoveryStateMachine};
use crate::scheduler::{AdmissionState, GpuCapacity, GpuTarget, ScheduleRequest, VramScheduler};
use crate::sysfs::{SysfsAerMonitor, SysfsLinkMonitor};
use crate::warning::{WarningStateMachine, WarningTrigger};
use crate::web::sse::{BroadcastEvent, SseBroadcaster};

/// A GPU lease granted to an application.
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
    pub active_leases: HashMap<String, GpuLease>,
    pub recovery_active: bool,
    pub remote_gpus: Vec<RegisteredRemoteGpu>,
}

/// The monitoring orchestrator. Spawns all monitoring tasks and
/// aggregates results into shared state.
pub struct MonitorOrchestrator {
    config: Arc<Config>,
    db: EventDb,
    state: Arc<Mutex<MonitorState>>,
    cancel: CancellationToken,
    sse: Option<SseBroadcaster>,
}

impl MonitorOrchestrator {
    pub fn new(
        config: Config,
        db: EventDb,
        cancel: CancellationToken,
    ) -> Self {
        let config = Arc::new(config);

        let warning_machine =
            WarningStateMachine::new(config.gpu.warning_cooldown_seconds);

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
        }
    }

    /// Set the SSE broadcaster for real-time event delivery.
    pub fn set_sse_broadcaster(&mut self, sse: SseBroadcaster) {
        self.sse = Some(sse);
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
        tokio::spawn(async move {
            Self::gpu_polling_loop(
                gpu_state,
                gpu_config,
                gpu_cancel,
                gpu_sse,
                gpu_trigger_tx,
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
            Self::retention_loop(
                db_clone,
                retention_config,
                retention_cancel,
            )
            .await;
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
                state.health_score.record_event(HealthEventKind::PcieTransient);
            }
            // FIX 11: CmpltToPattern und GpuProgressError ebenfalls als PCIe-Transient werten
            t if *t == WarningTrigger::CmpltToPattern || *t == WarningTrigger::GpuProgressError => {
                state.health_score.record_event(HealthEventKind::PcieTransient);
            }
            _ => {}
        }

        if let Some(new_level) = state.warning_machine.process_trigger(trigger) {
            // Log the level change
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
                    if new_level >= WarningLevel::Red { "urgent" } else { "high" },
                );
            }

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
            for action in &actions {
                info!(
                    "Migration-Aktion: {} — {:?} (Prio {})",
                    action.pipeline_name, action.action, action.priority
                );
            }

            // Gap 3: Try pressure reduction before full recovery at Orange
            if new_level >= WarningLevel::Orange && !state.recovery_active {
                state.recovery_active = true;
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
    ) {
        let gpu_monitor = NvidiaSmiMonitor::new(config.gpu.nvidia_smi_timeout_seconds);
        let mut consecutive_timeouts: u32 = 0;

        // Idle model tracking
        let mut last_gpu_activity: Option<std::time::Instant> = None;
        let mut models_loaded = false;

        // FIX 24: Power-Draw-Anomalie-Erkennung
        let mut power_baseline: Option<f64> = None;

        // Proactive monitoring state
        let mut smi_response_times: VecDeque<u128> = VecDeque::with_capacity(
            config.gpu.nvidia_smi_response_avg_window as usize,
        );
        let mut pstate_p4_since: Option<std::time::Instant> = None;
        let mut last_temp: Option<(u32, std::time::Instant)> = None;

        // Build Ollama control for auto-unloading
        let ollama_ctl = config.ollama.as_ref().and_then(|cfg| {
            if cfg.enabled {
                Some(crate::ollama::HttpOllamaControl::new(&cfg.host))
            } else {
                None
            }
        });

        info!("GPU-Poller gestartet (Intervall: {}s)", config.gpu.poll_interval_seconds);

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

                            // Track nvidia-smi response time
                            let window = config.gpu.nvidia_smi_response_avg_window as usize;
                            if smi_response_times.len() >= window {
                                smi_response_times.pop_front();
                            }
                            smi_response_times.push_back(response_ms);

                            // Check for slow single response -> health score penalty
                            if response_ms > 1000 {
                                let mut st = state.lock().await;
                                st.health_score.record_event(HealthEventKind::NvidiaSmiSlow);
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
                                    let _ = trigger_tx.send(WarningTrigger::NvidiaSmiSlow).await;
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

                            // eGPU-specific proactive checks
                            if let Some(egpu_status) = gpus.iter().find(|g| g.pci_address == config.gpu.egpu_pci_address) {
                                // --- P-State throttling detection ---
                                let pstate_num = egpu_status.pstate
                                    .trim_start_matches('P')
                                    .parse::<u32>()
                                    .unwrap_or(0);

                                if pstate_num >= config.gpu.pstate_throttle_threshold {
                                    match pstate_p4_since {
                                        Some(since) => {
                                            let sustained = since.elapsed().as_secs();
                                            if sustained >= config.gpu.pstate_throttle_sustained_seconds {
                                                warn!(
                                                    "P-State P{} sustained {}s (Schwelle: {}s)",
                                                    pstate_num, sustained,
                                                    config.gpu.pstate_throttle_sustained_seconds
                                                );
                                                let _ = trigger_tx.send(WarningTrigger::PstateThrottle).await;
                                                let mut st = state.lock().await;
                                                st.health_score.record_event(HealthEventKind::PstateAnomaly);
                                            }

                                            // P8 with active workloads is especially concerning
                                            if pstate_num >= 8 && egpu_status.utilization_gpu_percent > 0 {
                                                warn!(
                                                    "P-State P{} bei aktiver GPU-Nutzung ({}%) — Anomalie",
                                                    pstate_num, egpu_status.utilization_gpu_percent
                                                );
                                                let _ = trigger_tx.send(WarningTrigger::PstateThrottle).await;
                                                let mut st = state.lock().await;
                                                st.health_score.record_event(HealthEventKind::PstateAnomaly);
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
                                if let Some((prev_temp, prev_time)) = last_temp {
                                    let elapsed_min = prev_time.elapsed().as_secs_f64() / 60.0;
                                    if elapsed_min > 0.01 {
                                        let gradient = if current_temp > prev_temp {
                                            (current_temp - prev_temp) as f64 / elapsed_min
                                        } else {
                                            0.0
                                        };

                                        if gradient > config.gpu.thermal_gradient_warning_c_per_min
                                            && egpu_status.utilization_gpu_percent > 50
                                        {
                                            warn!(
                                                "Thermischer Gradient {:.1}°C/min > {:.1}°C/min (Temp: {}°C, Last: {}°C)",
                                                gradient,
                                                config.gpu.thermal_gradient_warning_c_per_min,
                                                current_temp,
                                                prev_temp,
                                            );
                                            let mut st = state.lock().await;
                                            st.health_score.record_event(HealthEventKind::TemperatureSpike);
                                        }
                                    }
                                }
                                last_temp = Some((current_temp, std::time::Instant::now()));

                                // --- Absolute thermal thresholds ---
                                if current_temp >= config.gpu.thermal_critical_temp_c {
                                    let _ = trigger_tx.send(WarningTrigger::ThermalCritical).await;
                                    let mut st = state.lock().await;
                                    st.health_score.record_event(HealthEventKind::TemperatureSpike);
                                } else if current_temp >= config.gpu.thermal_throttle_temp_c {
                                    let _ = trigger_tx.send(WarningTrigger::ThermalThrottle).await;
                                }

                                // FIX 24: Power-Draw-Anomalie-Erkennung
                                let power_w = egpu_status.power_draw_w;
                                let tdp_w = 250.0_f64; // RTX 5070 Ti TDP

                                // Baseline aktualisieren (gleitender Durchschnitt)
                                power_baseline = Some(match power_baseline {
                                    Some(baseline) => baseline * 0.95 + power_w * 0.05,
                                    None => power_w,
                                });

                                // Ploetzlicher Abfall auf < 5W bei aktiver GPU (PCIe-Link-Problem)
                                if power_w < 5.0 && egpu_status.utilization_gpu_percent > 10 {
                                    warn!(
                                        "Power-Draw-Anomalie: {:.1}W bei {}% Auslastung — moeglicherweise PCIe-Link-Problem",
                                        power_w, egpu_status.utilization_gpu_percent
                                    );
                                    let mut st = state.lock().await;
                                    st.health_score.record_event(HealthEventKind::PcieTransient);
                                }

                                // Dauerhafte Ueberschreitung von TDP * 1.1 (Thermisches Risiko)
                                if power_w > tdp_w * 1.1 {
                                    warn!(
                                        "Power-Draw {:.1}W ueberschreitet TDP*1.1 ({:.1}W) — thermisches Risiko",
                                        power_w, tdp_w * 1.1
                                    );
                                    let mut st = state.lock().await;
                                    st.health_score.record_event(HealthEventKind::TemperatureSpike);
                                }

                                // Track GPU activity for idle detection
                                if egpu_status.utilization_gpu_percent > 0 {
                                    last_gpu_activity = Some(std::time::Instant::now());
                                }
                            }

                            // Idle tracking and thermal protection for eGPU (Ollama)
                            if let Some(ref ollama_cfg) = config.ollama {
                                let egpu = gpus.iter().find(|g| g.pci_address == config.gpu.egpu_pci_address);
                                if let Some(egpu_status) = egpu {
                                    // Thermal protection: unload models if GPU is too hot
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

                            // Update scheduler compute utilization
                            {
                                let mut st = state.lock().await;
                                for gpu in &gpus {
                                    let target = if gpu.pci_address == config.gpu.egpu_pci_address {
                                        GpuTarget::Egpu
                                    } else {
                                        GpuTarget::Internal
                                    };
                                    st.scheduler.set_compute_utilization(target, gpu.utilization_gpu_percent);
                                }
                                st.gpu_status = gpus.clone();
                            }

                            // Health score tick + SSE broadcast
                            {
                                let mut st = state.lock().await;
                                if let Some(trigger) = st.health_score.tick() {
                                    drop(st);
                                    let _ = trigger_tx.send(trigger).await;
                                } else {
                                    // Broadcast health score via SSE
                                    if let Some(ref sse) = sse {
                                        sse.send(BroadcastEvent::HealthScore(
                                            st.health_score.summary(),
                                        ));
                                    }
                                }
                            }

                            // Broadcast GPU status SSE event
                            if let Some(ref sse) = sse {
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

                    // Query Ollama models if configured
                    if let Some(ref ollama_cfg) = config.ollama
                        && ollama_cfg.enabled
                    {
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
        if let Some(ref ollama_cfg) = config.ollama {
            if ollama_cfg.enabled {
                let ctl = crate::ollama::HttpOllamaControl::new(&ollama_cfg.host);
                Self::unload_all_ollama_models(&ctl, &state).await;
            }
        }

        // Step 3: Wait for pressure to reduce
        let wait_secs = config.gpu.pressure_reduction_wait_seconds;
        info!("Druckreduktion: Warte {}s", wait_secs);
        tokio::time::sleep(std::time::Duration::from_secs(wait_secs)).await;

        // Step 4: Check if nvidia-smi responds quickly
        let gpu_monitor = NvidiaSmiMonitor::new(config.gpu.nvidia_smi_timeout_seconds);
        let start = std::time::Instant::now();
        match gpu_monitor.query_all().await {
            Ok(_) => {
                let response_ms = start.elapsed().as_millis();
                let threshold = config.gpu.nvidia_smi_slow_threshold_ms as u128;

                if response_ms < threshold {
                    info!(
                        "Druckreduktion erfolgreich: nvidia-smi antwortet in {}ms (< {}ms)",
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
    async fn lease_expiry_loop(
        state: Arc<Mutex<MonitorState>>,
        cancel: CancellationToken,
    ) {
        let interval = std::time::Duration::from_secs(5);
        loop {
            tokio::select! {
                _ = cancel.cancelled() => {
                    info!("Lease-Expiry-Task beendet");
                    return;
                }
                _ = tokio::time::sleep(interval) => {
                    let now = Utc::now();
                    let mut st = state.lock().await;
                    let expired: Vec<String> = st
                        .active_leases
                        .iter()
                        .filter(|(_, lease)| lease.expires_at <= now)
                        .map(|(id, _)| id.clone())
                        .collect();

                    for id in expired {
                        if let Some(lease) = st.active_leases.remove(&id) {
                            info!(
                                "Lease {} abgelaufen (Pipeline: {}, VRAM: {} MB)",
                                id, lease.pipeline, lease.vram_mb
                            );
                        }
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

        let mut rsm = RecoveryStateMachine::new(
            db.clone(),
            config.recovery.reset_cooldown_seconds,
        );

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

    async fn retention_loop(
        db: EventDb,
        config: Arc<Config>,
        cancel: CancellationToken,
    ) {
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

                    let mut state = state.lock().await;
                    if let Some(new_level) = state.warning_machine.try_step_down() {
                        state.scheduler.set_warning_level(new_level);

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

    let warning_level = st.warning_machine.current_level();

    // Try to find a GPU with available VRAM
    let egpu_available = st.scheduler.vram_available(GpuTarget::Egpu);
    let internal_available = st.scheduler.vram_available(GpuTarget::Internal);

    let target = if warning_level < WarningLevel::Yellow && egpu_available >= vram_mb {
        Some(GpuTarget::Egpu)
    } else if internal_available >= vram_mb {
        Some(GpuTarget::Internal)
    } else {
        None
    };

    match target {
        Some(gpu_target) => {
            let now = Utc::now();
            let lease_id = Uuid::new_v4().to_string();
            let gpu_device = match gpu_target {
                GpuTarget::Egpu => config.gpu.egpu_pci_address.clone(),
                GpuTarget::Internal => config.gpu.internal_pci_address.clone(),
            };

            let lease = GpuLease {
                lease_id: lease_id.clone(),
                pipeline,
                gpu_device,
                gpu_uuid: lease_id.clone(),
                vram_mb,
                workload_type,
                acquired_at: now,
                expires_at: now + chrono::Duration::seconds(duration_seconds as i64),
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
pub fn release_gpu_lease(
    state: &mut MonitorState,
    lease_id: &str,
) -> bool {
    state.active_leases.remove(lease_id).is_some()
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
