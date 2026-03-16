use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::Json;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::db::Severity;
use crate::monitor::{acquire_gpu_lease, release_gpu_lease};
use crate::scheduler::{AdmissionState, GpuTarget};
use crate::web::sse::BroadcastEvent;
use crate::web::AppState;

// ─── Request / Response types ────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct EventsQuery {
    pub limit: Option<u32>,
    #[serde(rename = "type")]
    pub event_type: Option<String>,
    pub since: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct AuditLogQuery {
    pub limit: Option<u32>,
    pub since: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct DryRunQuery {
    pub dry_run: Option<bool>,
}

#[derive(Debug, Deserialize)]
pub struct PriorityBody {
    pub priority: u32,
}

#[derive(Debug, Deserialize)]
pub struct AssignBody {
    pub gpu_device: String,
}

#[derive(Debug, Deserialize)]
pub struct ConfirmBody {
    pub confirm: Option<bool>,
}

#[derive(Debug, Deserialize)]
pub struct WorkloadUpdateBody {
    pub workload_type: Option<String>,
    pub vram_mb: Option<u64>,
    pub status: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct UnloadModelBody {
    pub model: String,
}

#[derive(Debug, Deserialize)]
pub struct AdmissionBody {
    pub action: String,
}

#[derive(Debug, Deserialize)]
pub struct GpuAcquireBody {
    pub pipeline: String,
    pub workload_type: String,
    pub vram_mb: u64,
    #[serde(default = "default_lease_duration")]
    pub duration_seconds: u64,
}

fn default_lease_duration() -> u64 {
    3600
}

#[derive(Debug, Deserialize)]
pub struct GpuReleaseBody {
    pub lease_id: String,
}

#[derive(Debug, Serialize)]
pub struct ApiError {
    pub error: String,
    pub code: u16,
}

#[derive(Debug, Serialize)]
pub struct DryRunResponse {
    pub dry_run: bool,
    pub impact: String,
    pub would_affect: Vec<String>,
}

// ─── Helpers ─────────────────────────────────────────────────────────────

fn api_error(status: StatusCode, msg: impl Into<String>) -> (StatusCode, Json<ApiError>) {
    let msg = msg.into();
    (
        status,
        Json(ApiError {
            error: msg,
            code: status.as_u16(),
        }),
    )
}

fn is_dry_run(q: &DryRunQuery) -> bool {
    q.dry_run.unwrap_or(false)
}

// ─── GET /api/status ─────────────────────────────────────────────────────

#[derive(Debug, Serialize)]
pub struct StatusResponse {
    pub daemon: DaemonStatus,
    pub gpus: Vec<GpuInfo>,
    pub remote_gpus: Vec<RemoteGpuInfo>,
    pub health_score: HealthScoreInfo,
}

#[derive(Debug, Serialize)]
pub struct DaemonStatus {
    pub version: String,
    pub uptime_seconds: u64,
    pub warning_level: String,
    pub egpu_admission_state: String,
    pub scheduler_queue_length: usize,
    pub recovery_active: bool,
    pub recovery_stage: Option<String>,
    pub mode: String,
    pub config_schema_version: u32,
}

#[derive(Debug, Serialize)]
pub struct HealthScoreInfo {
    pub score: f64,
    pub warned_low: bool,
    pub warned_critical: bool,
    pub event_count: usize,
    pub recent_events: Vec<serde_json::Value>,
}

#[derive(Debug, Serialize)]
pub struct GpuInfo {
    pub pci_address: String,
    pub nvidia_index: Option<u32>,
    pub gpu_uuid: String,
    pub name: String,
    #[serde(rename = "type")]
    pub gpu_type: String,
    pub temperature_c: u32,
    pub utilization_gpu_percent: u32,
    pub memory_used_mb: u64,
    pub memory_total_mb: u64,
    pub power_draw_w: f64,
    pub pstate: String,
    pub thunderbolt_status: Option<String>,
    pub aer_nonfatal_count: Option<u64>,
    pub pcie_link_speed: Option<String>,
    pub pcie_link_width: Option<u8>,
    pub pcie_tx_kbps: Option<u64>,
    pub pcie_rx_kbps: Option<u64>,
    pub bandwidth_utilization_percent: Option<f64>,
    pub cuda_watchdog_status: Option<String>,
    pub status: String,
}

#[derive(Debug, Serialize)]
pub struct RemoteGpuInfo {
    pub name: String,
    pub host: String,
    pub gpu_name: String,
    pub vram_mb: u64,
    pub status: String,
    pub latency_ms: Option<u64>,
    pub last_seen: Option<DateTime<Utc>>,
}

pub async fn get_status(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let cfg = state.config.load();
    let monitor_state = state.monitor_state.lock().await;
    let uptime = state.started_at.elapsed().as_secs();

    let warning_level = monitor_state.warning_machine.current_level();
    let queue_len = monitor_state.scheduler.queue().len();

    let admission = monitor_state.scheduler.effective_admission_state();

    let recovery_state = state.db.load_recovery_state().await.ok().flatten();
    let recovery_active = recovery_state.is_some() || monitor_state.recovery_active;
    let recovery_stage = recovery_state.map(|r| r.stage);

    let daemon = DaemonStatus {
        version: env!("CARGO_PKG_VERSION").to_string(),
        uptime_seconds: uptime,
        warning_level: format!("{warning_level}"),
        egpu_admission_state: admission.to_string(),
        scheduler_queue_length: queue_len,
        recovery_active,
        recovery_stage,
        mode: "normal".to_string(),
        config_schema_version: cfg.schema_version,
    };

    // Gap 5: Build GPU info from MonitorState.gpu_status (populated by GPU poller)
    let gpus: Vec<GpuInfo> = monitor_state
        .gpu_status
        .iter()
        .map(|g| {
            let gpu_type = if g.pci_address == cfg.gpu.egpu_pci_address {
                "egpu"
            } else if g.pci_address == cfg.gpu.internal_pci_address {
                "internal"
            } else {
                "unknown"
            };

            // Enrich with PCIe throughput data from MonitorState
            let (pcie_tx, pcie_rx, bw_util) =
                if let Some(tp) = monitor_state.pcie_throughput.get(&g.pci_address) {
                    let max_throughput_kbps: u64 = 1_000_000;
                    let util = (tp.tx_kbps + tp.rx_kbps) as f64
                        / max_throughput_kbps as f64
                        * 100.0;
                    (Some(tp.tx_kbps), Some(tp.rx_kbps), Some(util))
                } else {
                    (None, None, None)
                };

            GpuInfo {
                pci_address: g.pci_address.clone(),
                nvidia_index: g.nvidia_index,
                gpu_uuid: g.gpu_uuid.clone(),
                name: g.name.clone(),
                gpu_type: gpu_type.to_string(),
                temperature_c: g.temperature_c,
                utilization_gpu_percent: g.utilization_gpu_percent,
                memory_used_mb: g.memory_used_mb,
                memory_total_mb: g.memory_total_mb,
                power_draw_w: g.power_draw_w,
                pstate: g.pstate.clone(),
                thunderbolt_status: None,
                aer_nonfatal_count: None,
                pcie_link_speed: None,
                pcie_link_width: None,
                pcie_tx_kbps: pcie_tx,
                pcie_rx_kbps: pcie_rx,
                bandwidth_utilization_percent: bw_util,
                cuda_watchdog_status: None,
                status: format!("{:?}", g.status).to_lowercase(),
            }
        })
        .collect();

    // Remote GPUs: Merge konfigurierte Eintraege mit registrierten Knoten.
    let mut remote_gpus: Vec<RemoteGpuInfo> = Vec::new();

    let registered_names: Vec<String> = monitor_state
        .remote_gpus
        .iter()
        .map(|g| g.name.clone())
        .collect();

    for g in &monitor_state.remote_gpus {
        remote_gpus.push(RemoteGpuInfo {
            name: g.name.clone(),
            host: g.host.clone(),
            gpu_name: g.gpu_name.clone(),
            vram_mb: g.vram_mb,
            status: g.status.clone(),
            latency_ms: g.latency_ms.map(u64::from),
            last_seen: Some(g.last_heartbeat),
        });
    }

    for r in &cfg.remote_gpu {
        if !registered_names.contains(&r.name) {
            remote_gpus.push(RemoteGpuInfo {
                name: r.name.clone(),
                host: r.host.clone(),
                gpu_name: r.gpu_name.clone(),
                vram_mb: r.vram_mb,
                status: "offline".to_string(),
                latency_ms: None,
                last_seen: None,
            });
        }
    }

    // Health Score summary
    let hs_summary = monitor_state.health_score.summary();
    let health_score = HealthScoreInfo {
        score: hs_summary["score"].as_f64().unwrap_or(100.0),
        warned_low: hs_summary["warned_low"].as_bool().unwrap_or(false),
        warned_critical: hs_summary["warned_critical"].as_bool().unwrap_or(false),
        event_count: hs_summary["event_count"].as_u64().unwrap_or(0) as usize,
        recent_events: hs_summary["recent_events"]
            .as_array()
            .cloned()
            .unwrap_or_default(),
    };

    Json(StatusResponse {
        daemon,
        gpus,
        remote_gpus,
        health_score,
    })
}

// ─── GET /api/pipelines ──────────────────────────────────────────────────

#[derive(Debug, Serialize)]
pub struct PipelineInfo {
    pub project: String,
    pub container: String,
    pub gpu_device: String,
    pub gpu_type: String,
    pub priority: u32,
    pub vram_estimate_mb: u64,
    pub actual_vram_mb: u64,
    pub workload_types: Vec<String>,
    pub status: String,
    pub decision_reason: String,
    pub assignment_source: String,
    pub queue_position: Option<usize>,
    pub blocked_by: Option<String>,
    pub vram_summary: VramSummary,
}

#[derive(Debug, Serialize)]
pub struct VramSummary {
    pub estimated_mb: u64,
    pub actual_mb: u64,
}

pub async fn get_pipelines(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let config = state.config.load();
    let monitor_state = state.monitor_state.lock().await;
    let mut pipelines = Vec::new();

    for cfg in &config.pipeline {
        let (gpu_type, status, actual_vram, decision_reason, assignment_source) =
            if let Some(assignment) = monitor_state.scheduler.assignments().get(&cfg.container) {
                let gpu_type = match assignment.target {
                    GpuTarget::Egpu => "egpu",
                    GpuTarget::Internal => "internal",
                };
                (
                    gpu_type.to_string(),
                    "assigned".to_string(),
                    assignment.actual_vram_mb,
                    "scheduler".to_string(),
                    if assignment.target == assignment.preferred_target {
                        "preferred"
                    } else {
                        "fallback"
                    }
                    .to_string(),
                )
            } else {
                ("none".to_string(), "unassigned".to_string(), 0u64, "n/a".to_string(), "n/a".to_string())
            };

        let queue_position = monitor_state
            .scheduler
            .queue()
            .iter()
            .position(|r| r.name == cfg.container);

        let warning_level = monitor_state.warning_machine.current_level();
        let blocked_by =
            if warning_level >= egpu_manager_common::gpu::WarningLevel::Yellow
                && cfg.gpu_device == config.gpu.egpu_pci_address
            {
                Some(format!("warning_level:{warning_level}"))
            } else {
                None
            };

        pipelines.push(PipelineInfo {
            project: cfg.project.clone(),
            container: cfg.container.clone(),
            gpu_device: cfg.gpu_device.clone(),
            gpu_type,
            priority: cfg.gpu_priority,
            vram_estimate_mb: cfg.vram_estimate_mb,
            actual_vram_mb: actual_vram,
            workload_types: cfg.workload_types.clone(),
            status,
            decision_reason,
            assignment_source,
            queue_position,
            blocked_by,
            vram_summary: VramSummary {
                estimated_mb: cfg.vram_estimate_mb,
                actual_mb: actual_vram,
            },
        });
    }

    Json(pipelines)
}

// ─── GET /api/pipelines/:container ───────────────────────────────────────

pub async fn get_pipeline(
    State(state): State<Arc<AppState>>,
    Path(container): Path<String>,
) -> impl IntoResponse {
    let config = state.config.load();
    let pipeline_cfg = config
        .pipeline
        .iter()
        .find(|p| p.container == container);

    let Some(cfg) = pipeline_cfg else {
        return api_error(StatusCode::NOT_FOUND, format!("Pipeline '{container}' nicht gefunden"))
            .into_response();
    };

    let monitor_state = state.monitor_state.lock().await;
    let (gpu_type, status, actual_vram, decision_reason, assignment_source) =
        if let Some(assignment) = monitor_state.scheduler.assignments().get(&cfg.container) {
            let gpu_type = match assignment.target {
                GpuTarget::Egpu => "egpu",
                GpuTarget::Internal => "internal",
            };
            (
                gpu_type.to_string(),
                "assigned".to_string(),
                assignment.actual_vram_mb,
                "scheduler".to_string(),
                if assignment.target == assignment.preferred_target {
                    "preferred"
                } else {
                    "fallback"
                }
                .to_string(),
            )
        } else {
            ("none".to_string(), "unassigned".to_string(), 0u64, "n/a".to_string(), "n/a".to_string())
        };

    let queue_position = monitor_state
        .scheduler
        .queue()
        .iter()
        .position(|r| r.name == cfg.container);

    let warning_level = monitor_state.warning_machine.current_level();
    let blocked_by =
        if warning_level >= egpu_manager_common::gpu::WarningLevel::Yellow
            && cfg.gpu_device == config.gpu.egpu_pci_address
        {
            Some(format!("warning_level:{warning_level}"))
        } else {
            None
        };

    let info = PipelineInfo {
        project: cfg.project.clone(),
        container: cfg.container.clone(),
        gpu_device: cfg.gpu_device.clone(),
        gpu_type,
        priority: cfg.gpu_priority,
        vram_estimate_mb: cfg.vram_estimate_mb,
        actual_vram_mb: actual_vram,
        workload_types: cfg.workload_types.clone(),
        status,
        decision_reason,
        assignment_source,
        queue_position,
        blocked_by,
        vram_summary: VramSummary {
            estimated_mb: cfg.vram_estimate_mb,
            actual_mb: actual_vram,
        },
    };

    Json(info).into_response()
}

// ─── PUT /api/pipelines/:container/priority ──────────────────────────────

pub async fn put_pipeline_priority(
    State(state): State<Arc<AppState>>,
    Path(container): Path<String>,
    Query(dry_run): Query<DryRunQuery>,
    Json(body): Json<PriorityBody>,
) -> impl IntoResponse {
    if body.priority == 0 || body.priority > 5 {
        return api_error(
            StatusCode::BAD_REQUEST,
            "Prioritaet muss zwischen 1 und 5 liegen",
        )
        .into_response();
    }

    if is_dry_run(&dry_run) {
        return Json(DryRunResponse {
            dry_run: true,
            impact: format!(
                "Pipeline '{container}' Prioritaet wuerde auf {} geaendert",
                body.priority
            ),
            would_affect: vec![container],
        })
        .into_response();
    }

    let mut monitor_state = state.monitor_state.lock().await;
    if let Some(assignment) = monitor_state
        .scheduler
        .assignments_mut()
        .get_mut(&container)
    {
        let old_priority = assignment.priority;
        assignment.priority = body.priority;

        state
            .db
            .log_event(
                "api.priority_change",
                Severity::Info,
                &format!(
                    "Pipeline '{container}' Prioritaet: {old_priority} -> {}",
                    body.priority
                ),
                Some(serde_json::json!({
                    "container": container,
                    "old_priority": old_priority,
                    "new_priority": body.priority,
                })),
            )
            .await
            .ok();

        state.sse.send(BroadcastEvent::PipelineChange(
            serde_json::json!({
                "container": container,
                "action": "priority_change",
                "priority": body.priority,
            }),
        ));

        Json(serde_json::json!({
            "ok": true,
            "container": container,
            "old_priority": old_priority,
            "new_priority": body.priority,
        }))
        .into_response()
    } else {
        api_error(
            StatusCode::NOT_FOUND,
            format!("Pipeline '{container}' nicht gefunden"),
        )
        .into_response()
    }
}

// ─── POST /api/pipelines/:container/assign ───────────────────────────────

pub async fn post_pipeline_assign(
    State(state): State<Arc<AppState>>,
    Path(container): Path<String>,
    Query(dry_run): Query<DryRunQuery>,
    Json(body): Json<AssignBody>,
) -> impl IntoResponse {
    let config = state.config.load();
    let target = if body.gpu_device == config.gpu.egpu_pci_address {
        GpuTarget::Egpu
    } else if body.gpu_device == config.gpu.internal_pci_address {
        GpuTarget::Internal
    } else {
        return api_error(
            StatusCode::BAD_REQUEST,
            format!("Unbekannte GPU-Adresse: {}", body.gpu_device),
        )
        .into_response();
    };

    if is_dry_run(&dry_run) {
        return Json(DryRunResponse {
            dry_run: true,
            impact: format!("Pipeline '{container}' wuerde auf {target} zugewiesen"),
            would_affect: vec![container],
        })
        .into_response();
    }

    let mut monitor_state = state.monitor_state.lock().await;
    if monitor_state.scheduler.migrate(&container, target) {
        state
            .db
            .log_event(
                "api.gpu_assign",
                Severity::Info,
                &format!("Pipeline '{container}' manuell auf {target} zugewiesen"),
                Some(serde_json::json!({
                    "container": container,
                    "gpu_device": body.gpu_device,
                    "target": format!("{target}"),
                })),
            )
            .await
            .ok();

        state.sse.send(BroadcastEvent::PipelineChange(
            serde_json::json!({
                "container": container,
                "action": "gpu_assign",
                "target": format!("{target}"),
            }),
        ));

        Json(serde_json::json!({
            "ok": true,
            "container": container,
            "target": format!("{target}"),
        }))
        .into_response()
    } else {
        api_error(
            StatusCode::NOT_FOUND,
            format!("Pipeline '{container}' nicht gefunden"),
        )
        .into_response()
    }
}

// ─── POST /api/pipelines/:container/workload-update ──────────────────────

pub async fn post_workload_update(
    State(state): State<Arc<AppState>>,
    Path(container): Path<String>,
    Json(body): Json<WorkloadUpdateBody>,
) -> impl IntoResponse {
    let mut monitor_state = state.monitor_state.lock().await;

    if let Some(vram) = body.vram_mb {
        monitor_state.scheduler.update_actual_vram(&container, vram);
    }

    state
        .db
        .log_event(
            "api.workload_update",
            Severity::Debug,
            &format!("Workload-Update fuer '{container}'"),
            Some(serde_json::json!({
                "container": container,
                "workload_type": body.workload_type,
                "vram_mb": body.vram_mb,
                "status": body.status,
            })),
        )
        .await
        .ok();

    Json(serde_json::json!({"ok": true}))
}

// ─── POST /api/gpu/acquire (Gap 4) ──────────────────────────────────────

pub async fn post_gpu_acquire(
    State(state): State<Arc<AppState>>,
    Json(body): Json<GpuAcquireBody>,
) -> impl IntoResponse {
    let config = state.config.load();
    let result = acquire_gpu_lease(
        &state.monitor_state,
        &config,
        body.pipeline.clone(),
        body.workload_type.clone(),
        body.vram_mb,
        body.duration_seconds,
    )
    .await;

    match result {
        Ok(lease) => {
            let warning_level = {
                let st = state.monitor_state.lock().await;
                st.warning_machine.current_level()
            };

            state
                .db
                .log_event(
                    "api.gpu_acquire",
                    Severity::Info,
                    &format!(
                        "GPU-Lease erteilt: {} ({} MB VRAM auf {})",
                        lease.lease_id, lease.vram_mb, lease.gpu_device
                    ),
                    Some(serde_json::json!({
                        "lease_id": lease.lease_id,
                        "pipeline": body.pipeline,
                        "vram_mb": body.vram_mb,
                        "gpu_device": lease.gpu_device,
                    })),
                )
                .await
                .ok();

            Json(serde_json::json!({
                "granted": true,
                "gpu_device": lease.gpu_device,
                "gpu_uuid": lease.gpu_uuid,
                "lease_id": lease.lease_id,
                "warning_level": format!("{warning_level}"),
                "expires_at": lease.expires_at.to_rfc3339(),
            }))
            .into_response()
        }
        Err(queue_position) => {
            let warning_level = {
                let st = state.monitor_state.lock().await;
                st.warning_machine.current_level()
            };

            Json(serde_json::json!({
                "granted": false,
                "queue_position": queue_position,
                "warning_level": format!("{warning_level}"),
                "reason": "Nicht genuegend VRAM verfuegbar",
            }))
            .into_response()
        }
    }
}

// ─── POST /api/gpu/release (Gap 4) ──────────────────────────────────────

pub async fn post_gpu_release(
    State(state): State<Arc<AppState>>,
    Json(body): Json<GpuReleaseBody>,
) -> impl IntoResponse {
    let mut monitor_state = state.monitor_state.lock().await;
    let released = release_gpu_lease(&mut monitor_state, &body.lease_id);

    if released {
        // Drop the lock before logging
        drop(monitor_state);

        state
            .db
            .log_event(
                "api.gpu_release",
                Severity::Info,
                &format!("GPU-Lease freigegeben: {}", body.lease_id),
                Some(serde_json::json!({
                    "lease_id": body.lease_id,
                })),
            )
            .await
            .ok();

        Json(serde_json::json!({
            "ok": true,
            "lease_id": body.lease_id,
        }))
        .into_response()
    } else {
        api_error(
            StatusCode::NOT_FOUND,
            format!("Lease '{}' nicht gefunden", body.lease_id),
        )
        .into_response()
    }
}

// ─── GET /api/gpu/recommend (Gap 4) ──────────────────────────────────────

pub async fn get_gpu_recommend(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let cfg = state.config.load();
    let monitor_state = state.monitor_state.lock().await;

    let warning_level = monitor_state.warning_machine.current_level();
    let egpu_available = monitor_state.scheduler.vram_available(GpuTarget::Egpu);
    let internal_available = monitor_state.scheduler.vram_available(GpuTarget::Internal);

    let recommended = if warning_level < egpu_manager_common::gpu::WarningLevel::Yellow
        && egpu_available > internal_available
    {
        "egpu"
    } else {
        "internal"
    };

    let recommended_device = if recommended == "egpu" {
        &cfg.gpu.egpu_pci_address
    } else {
        &cfg.gpu.internal_pci_address
    };

    // Get Ollama host if configured
    let ollama_host = cfg
        .ollama
        .as_ref()
        .filter(|o| o.enabled)
        .map(|o| o.host.clone());

    Json(serde_json::json!({
        "recommended_gpu": recommended,
        "recommended_device": recommended_device,
        "warning_level": format!("{warning_level}"),
        "egpu_vram_available_mb": egpu_available,
        "internal_vram_available_mb": internal_available,
        "ollama_host": ollama_host,
        "active_leases": monitor_state.active_leases.len(),
    }))
}

// ─── POST /api/egpu/admission ────────────────────────────────────────────

pub async fn post_egpu_admission(
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
    Json(body): Json<AdmissionBody>,
) -> impl IntoResponse {
    let cfg = state.config.load();
    if let Err(resp) = check_bearer_auth(&headers, &cfg) {
        return resp.into_response();
    }
    let new_state = match body.action.as_str() {
        "open" => AdmissionState::Open,
        "drain" => AdmissionState::Drain,
        "close" => AdmissionState::Closed,
        other => {
            return api_error(
                StatusCode::BAD_REQUEST,
                format!(
                    "Unbekannte Aktion: '{}'. Erlaubt: open, drain, close",
                    other
                ),
            )
            .into_response();
        }
    };

    let mut monitor_state = state.monitor_state.lock().await;
    let old_state = monitor_state.scheduler.admission_state();
    monitor_state.scheduler.set_admission_state(new_state);

    // Drop the lock before async logging
    drop(monitor_state);

    state
        .db
        .log_event(
            "api.egpu_admission",
            Severity::Warning,
            &format!(
                "eGPU-Admission geaendert: {} -> {}",
                old_state, new_state
            ),
            Some(serde_json::json!({
                "old_state": format!("{old_state}"),
                "new_state": format!("{new_state}"),
                "action": body.action,
            })),
        )
        .await
        .ok();

    state.sse.send(BroadcastEvent::PipelineChange(
        serde_json::json!({
            "action": "egpu_admission_change",
            "old_state": format!("{old_state}"),
            "new_state": format!("{new_state}"),
        }),
    ));

    Json(serde_json::json!({
        "ok": true,
        "old_state": format!("{old_state}"),
        "new_state": format!("{new_state}"),
    }))
    .into_response()
}

// ─── GET /api/ollama/status ──────────────────────────────────────────────

pub async fn get_ollama_status(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let cfg = state.config.load();
    let Some(ref ollama_cfg) = cfg.ollama else {
        return api_error(StatusCode::NOT_FOUND, "Ollama nicht konfiguriert").into_response();
    };

    if !ollama_cfg.enabled {
        return Json(serde_json::json!({
            "enabled": false,
            "models": [],
        }))
        .into_response();
    }

    // Return cached Ollama models from MonitorState if available
    let monitor_state = state.monitor_state.lock().await;
    if !monitor_state.ollama_models.is_empty() {
        let total_vram: u64 = monitor_state.ollama_models.iter().map(|m| m.size_vram_bytes).sum();
        return Json(serde_json::json!({
            "enabled": true,
            "host": ollama_cfg.host,
            "models": monitor_state.ollama_models,
            "total_vram_bytes": total_vram,
            "total_vram_mb": total_vram / 1024 / 1024,
        }))
        .into_response();
    }
    drop(monitor_state);

    // Fallback: query directly
    match crate::nvidia::query_ollama_models(&ollama_cfg.host).await {
        Ok(models) => {
            let total_vram: u64 = models.iter().map(|m| m.size_vram_bytes).sum();
            Json(serde_json::json!({
                "enabled": true,
                "host": ollama_cfg.host,
                "models": models,
                "total_vram_bytes": total_vram,
                "total_vram_mb": total_vram / 1024 / 1024,
            }))
            .into_response()
        }
        Err(e) => {
            api_error(StatusCode::BAD_GATEWAY, format!("Ollama nicht erreichbar: {e}"))
                .into_response()
        }
    }
}

// ─── POST /api/ollama/unload ─────────────────────────────────────────────

pub async fn post_ollama_unload(
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
    Query(dry_run): Query<DryRunQuery>,
    Json(body): Json<UnloadModelBody>,
) -> impl IntoResponse {
    let cfg = state.config.load();
    if let Err(resp) = check_bearer_auth(&headers, &cfg) {
        return resp.into_response();
    }
    let Some(ref ollama_cfg) = cfg.ollama else {
        return api_error(StatusCode::NOT_FOUND, "Ollama nicht konfiguriert").into_response();
    };

    if !ollama_cfg.enabled {
        return api_error(StatusCode::BAD_REQUEST, "Ollama ist deaktiviert").into_response();
    }

    if is_dry_run(&dry_run) {
        return Json(DryRunResponse {
            dry_run: true,
            impact: format!("Modell '{}' wuerde entladen", body.model),
            would_affect: vec![body.model],
        })
        .into_response();
    }

    // Unload via Ollama API (POST /api/generate with keep_alive=0)
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build();
    let Ok(client) = client else {
        return api_error(StatusCode::INTERNAL_SERVER_ERROR, "HTTP-Client Fehler").into_response();
    };

    let url = format!("{}/api/generate", ollama_cfg.host);
    let result = client
        .post(&url)
        .json(&serde_json::json!({
            "model": body.model,
            "keep_alive": 0,
        }))
        .send()
        .await;

    match result {
        Ok(resp) if resp.status().is_success() => {
            state
                .db
                .log_event(
                    "api.ollama_unload",
                    Severity::Info,
                    &format!("Ollama-Modell '{}' entladen", body.model),
                    Some(serde_json::json!({"model": body.model})),
                )
                .await
                .ok();

            Json(serde_json::json!({"ok": true, "model": body.model})).into_response()
        }
        Ok(resp) => {
            api_error(
                StatusCode::BAD_GATEWAY,
                format!("Ollama Fehler: HTTP {}", resp.status()),
            )
            .into_response()
        }
        Err(e) => {
            api_error(
                StatusCode::BAD_GATEWAY,
                format!("Ollama nicht erreichbar: {e}"),
            )
            .into_response()
        }
    }
}

// ─── GET /api/recovery/status ────────────────────────────────────────────

pub async fn get_recovery_status(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    match state.db.load_recovery_state().await {
        Ok(Some(recovery)) => Json(serde_json::json!({
            "active": true,
            "stage": recovery.stage,
            "status": recovery.status,
            "started_at": recovery.started_at,
            "updated_at": recovery.updated_at,
        }))
        .into_response(),
        Ok(None) => Json(serde_json::json!({
            "active": false,
            "stage": null,
            "status": "idle",
        }))
        .into_response(),
        Err(e) => {
            api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Recovery-Status nicht lesbar: {e}"),
            )
            .into_response()
        }
    }
}

// ─── POST /api/recovery/reset ────────────────────────────────────────────

pub async fn post_recovery_reset(
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
    Query(dry_run): Query<DryRunQuery>,
    Json(body): Json<ConfirmBody>,
) -> impl IntoResponse {
    let cfg = state.config.load();
    if let Err(resp) = check_bearer_auth(&headers, &cfg) {
        return resp.into_response();
    }
    if is_dry_run(&dry_run) {
        return Json(DryRunResponse {
            dry_run: true,
            impact: "PCIe-FLR-Reset wuerde ausgefuehrt".to_string(),
            would_affect: vec![cfg.gpu.egpu_pci_address.clone()],
        })
        .into_response();
    }

    if body.confirm != Some(true) {
        return api_error(
            StatusCode::BAD_REQUEST,
            "Bestaetigung erforderlich: {\"confirm\": true}",
        )
        .into_response();
    }

    state
        .db
        .log_event(
            "api.recovery_reset",
            Severity::Warning,
            "Manueller PCIe-Reset angefordert via API",
            None,
        )
        .await
        .ok();

    state.sse.send(BroadcastEvent::RecoveryStage(
        serde_json::json!({
            "action": "manual_reset_requested",
            "stage": "flr_reset",
        }),
    ));

    Json(serde_json::json!({
        "ok": true,
        "message": "PCIe-Reset angefordert",
        "note": "Reset wird vom Recovery-System ausgefuehrt"
    }))
    .into_response()
}

// ─── POST /api/recovery/thunderbolt-reconnect ────────────────────────────

pub async fn post_thunderbolt_reconnect(
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
    Query(dry_run): Query<DryRunQuery>,
    Json(body): Json<ConfirmBody>,
) -> impl IntoResponse {
    let cfg = state.config.load();
    if let Err(resp) = check_bearer_auth(&headers, &cfg) {
        return resp.into_response();
    }
    if is_dry_run(&dry_run) {
        return Json(DryRunResponse {
            dry_run: true,
            impact: "Thunderbolt-Reauthorisierung wuerde ausgefuehrt".to_string(),
            would_affect: vec!["thunderbolt_device".to_string()],
        })
        .into_response();
    }

    if body.confirm != Some(true) {
        return api_error(
            StatusCode::BAD_REQUEST,
            "Bestaetigung erforderlich: {\"confirm\": true}",
        )
        .into_response();
    }

    state
        .db
        .log_event(
            "api.thunderbolt_reconnect",
            Severity::Warning,
            "Thunderbolt-Reauthorisierung angefordert via API",
            None,
        )
        .await
        .ok();

    state.sse.send(BroadcastEvent::RecoveryStage(
        serde_json::json!({
            "action": "thunderbolt_reconnect_requested",
            "stage": "tb_reauth",
        }),
    ));

    Json(serde_json::json!({
        "ok": true,
        "message": "Thunderbolt-Reconnect angefordert",
        "note": "Reauthorisierung wird vom Recovery-System ausgefuehrt"
    }))
    .into_response()
}

// ─── GET /api/events ─────────────────────────────────────────────────────

pub async fn get_events(
    State(state): State<Arc<AppState>>,
    Query(query): Query<EventsQuery>,
) -> impl IntoResponse {
    let limit = query.limit.unwrap_or(100).min(1000);

    match state.db.query_recent_events(limit).await {
        Ok(mut events) => {
            // Filter by type if specified
            if let Some(ref event_type) = query.event_type {
                events.retain(|e| e.event_type.starts_with(event_type.as_str()));
            }

            // Filter by since if specified
            if let Some(ref since_str) = query.since
                && let Ok(since) = DateTime::parse_from_rfc3339(since_str)
            {
                let since_utc = since.with_timezone(&Utc);
                events.retain(|e| e.timestamp >= since_utc);
            }

            Json(serde_json::json!({
                "events": events,
                "count": events.len(),
            }))
            .into_response()
        }
        Err(e) => api_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Events nicht lesbar: {e}"),
        )
        .into_response(),
    }
}

// ─── GET /api/events/stream (SSE) ────────────────────────────────────────

pub async fn get_events_stream(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    state.sse.subscribe()
}

// ─── GET /api/audit-log ──────────────────────────────────────────────────

pub async fn get_audit_log(
    State(state): State<Arc<AppState>>,
    Query(query): Query<AuditLogQuery>,
) -> impl IntoResponse {
    let limit = query.limit.unwrap_or(100).min(1000);

    match state.db.query_recent_events(limit).await {
        Ok(mut events) => {
            // Audit events are those from API actions
            events.retain(|e| e.event_type.starts_with("api."));

            if let Some(ref since_str) = query.since
                && let Ok(since) = DateTime::parse_from_rfc3339(since_str)
            {
                let since_utc = since.with_timezone(&Utc);
                events.retain(|e| e.timestamp >= since_utc);
            }

            Json(serde_json::json!({
                "entries": events,
                "count": events.len(),
            }))
            .into_response()
        }
        Err(e) => api_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Audit-Log nicht lesbar: {e}"),
        )
        .into_response(),
    }
}

// ─── GET /api/config ─────────────────────────────────────────────────────

pub async fn get_config(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    // Return config without secrets (token paths, etc.)
    let cfg = state.config.load();
    let mut config_json = serde_json::to_value(&**cfg).unwrap_or_default();

    // Redact sensitive fields
    if let Some(remote) = config_json.get_mut("remote")
        && let Some(obj) = remote.as_object_mut()
    {
        if obj.contains_key("token_path") {
            obj.insert(
                "token_path".to_string(),
                serde_json::Value::String("[REDACTED]".to_string()),
            );
        }
        if obj.contains_key("tls_key") {
            obj.insert(
                "tls_key".to_string(),
                serde_json::Value::String("[REDACTED]".to_string()),
            );
        }
    }

    Json(config_json)
}

// ─── POST /api/config/reload ─────────────────────────────────────────────

pub async fn post_config_reload(
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
    Query(dry_run): Query<DryRunQuery>,
) -> impl IntoResponse {
    let cfg = state.config.load();
    if let Err(resp) = check_bearer_auth(&headers, &cfg) {
        return resp.into_response();
    }
    drop(cfg); // Guard vor dem Swap freigeben
    if is_dry_run(&dry_run) {
        return Json(DryRunResponse {
            dry_run: true,
            impact: "Konfiguration wuerde neu geladen".to_string(),
            would_affect: vec!["config".to_string()],
        })
        .into_response();
    }

    // Log the reload request
    state
        .db
        .log_event(
            "api.config_reload",
            Severity::Info,
            "Konfigurations-Reload angefordert via API",
            None,
        )
        .await
        .ok();

    state.sse.send(BroadcastEvent::ConfigReload(
        serde_json::json!({"action": "reload_requested"}),
    ));

    // Konfigurations-Datei laden, validieren und hot-swappen
    let config_path = std::path::Path::new("/etc/egpu-manager/config.toml");
    match egpu_manager_common::config::Config::load(config_path) {
        Ok(new_config) => {
            // Neue Konfiguration atomar in den ArcSwap schreiben —
            // alle nachfolgenden Handler-Aufrufe sehen sofort die neue Config.
            state.config.store(Arc::new(new_config));

            tracing::info!("Konfiguration hot-reloaded via API");

            Json(serde_json::json!({
                "ok": true,
                "message": "Konfiguration geladen, validiert und hot-reloaded",
            }))
            .into_response()
        }
        Err(e) => {
            api_error(
                StatusCode::BAD_REQUEST,
                format!("Konfiguration ungueltig: {e}"),
            )
            .into_response()
        }
    }
}

// ─── System Stats ───────────────────────────────────────────────────────────

/// GET /api/system — Host system stats (CPU, RAM, load, uptime)
pub async fn get_system_stats() -> impl IntoResponse {
    let stats = crate::sysinfo::get_system_stats().await;
    Json(stats)
}

// ─── Setup Generator ────────────────────────────────────────────────────────

/// POST /api/setup/generate — Generate Windows Remote-Node setup ZIP
pub async fn post_setup_generate(
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    match crate::setup_generator::generate_setup_zip() {
        Ok(zip_bytes) => {
            state
                .db
                .log_event(
                    "api.setup_generate",
                    Severity::Info,
                    &format!(
                        "Windows-Setup-ZIP generiert ({} KB)",
                        zip_bytes.len() / 1024
                    ),
                    None,
                )
                .await
                .ok();

            (
                StatusCode::OK,
                [
                    (
                        axum::http::header::CONTENT_TYPE,
                        "application/zip",
                    ),
                    (
                        axum::http::header::CONTENT_DISPOSITION,
                        "attachment; filename=\"egpu-remote-setup.zip\"",
                    ),
                ],
                zip_bytes,
            )
                .into_response()
        }
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({
                "error": format!("ZIP-Generierung fehlgeschlagen: {e}")
            })),
        )
            .into_response(),
    }
}

/// GET /api/setup/instructions — Return installation instructions
pub async fn get_setup_instructions() -> impl IntoResponse {
    Json(serde_json::json!({
        "steps": [
            "1. ZIP herunterladen (Button oder POST /api/setup/generate)",
            "2. ZIP auf USB-Stick kopieren",
            "3. Am Windows-11-Rechner: ZIP nach C:\\egpu-remote\\setup\\ entpacken",
            "4. PowerShell als Administrator oeffnen",
            "5. cd C:\\egpu-remote\\setup\\egpu-remote-setup",
            "6. Set-ExecutionPolicy -Scope Process -ExecutionPolicy Bypass",
            "7. .\\install.ps1 ausfuehren",
            "8. Remote-GPU erscheint im NUC-Dashboard"
        ],
        "requirements": [
            "Windows 11",
            "Administratorrechte",
            "NVIDIA-Treiber >= 576.02",
            "20 GB freier Speicherplatz",
            "NUC erreichbar im Netzwerk"
        ]
    }))
}

// ─── LLM Gateway Endpoints ─────────────────────────────────────────────────

/// POST /api/llm/chat/completions — OpenAI-kompatible Chat-Completion via LLM Gateway
pub async fn post_llm_chat_completions(
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
    Json(request): Json<crate::llm::types::ChatCompletionRequest>,
) -> impl IntoResponse {
    let Some(ref router) = state.llm_router else {
        return api_error(StatusCode::SERVICE_UNAVAILABLE, "LLM Gateway nicht konfiguriert")
            .into_response();
    };

    let app_id = headers
        .get("x-app-id")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("unknown");

    match router.chat_completion(request, app_id).await {
        Ok(response) => Json(serde_json::to_value(response).unwrap()).into_response(),
        Err(err) => {
            let status = match err.error.r#type.as_str() {
                "rate_limit_error" => StatusCode::TOO_MANY_REQUESTS,
                "budget_exceeded" => StatusCode::PAYMENT_REQUIRED,
                "permission_error" => StatusCode::FORBIDDEN,
                "routing_error" => StatusCode::SERVICE_UNAVAILABLE,
                _ => StatusCode::BAD_GATEWAY,
            };
            (status, Json(serde_json::to_value(err).unwrap())).into_response()
        }
    }
}

/// GET /api/llm/providers — Liste aller konfigurierten LLM-Provider
pub async fn get_llm_providers(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let Some(ref router) = state.llm_router else {
        return api_error(StatusCode::SERVICE_UNAVAILABLE, "LLM Gateway nicht konfiguriert")
            .into_response();
    };

    let statuses = router.provider_status().await;
    Json(serde_json::json!({ "providers": statuses })).into_response()
}

/// GET /api/llm/usage/:app_id — Nutzungsstatistiken fuer eine App
pub async fn get_llm_usage(
    State(state): State<Arc<AppState>>,
    Path(app_id): Path<String>,
) -> impl IntoResponse {
    let Some(ref router) = state.llm_router else {
        return api_error(StatusCode::SERVICE_UNAVAILABLE, "LLM Gateway nicht konfiguriert")
            .into_response();
    };

    let summary = router.usage_for_app(&app_id).await;
    // FIX 16: DB-gestuetzte Nutzungsdaten koennten hier ergaenzend aus
    // state.db.query_monthly_usage(&app_id) geladen werden.
    Json(serde_json::to_value(summary).unwrap()).into_response()
}

/// GET /api/llm/health — Health-Check fuer das LLM Gateway
pub async fn get_llm_health(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let Some(ref router) = state.llm_router else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({
                "status": "disabled",
                "providers_count": 0,
                "healthy_count": 0,
            })),
        )
            .into_response();
    };

    let providers = router.provider_status().await;
    let any_healthy = providers.iter().any(|p| p.healthy);

    let status = if any_healthy {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };

    (
        status,
        Json(serde_json::json!({
            "status": if any_healthy { "ok" } else { "degraded" },
            "providers_count": providers.len(),
            "healthy_count": providers.iter().filter(|p| p.healthy).count(),
        })),
    )
        .into_response()
}

// ─── Bearer Token Auth Middleware ────────────────────────────────────────────

/// Prueft Bearer-Token-Authentifizierung fuer destruktive Endpunkte.
/// Gibt Ok(()) zurueck wenn auth bestanden oder nicht konfiguriert (token leer).
pub fn check_bearer_auth(
    headers: &axum::http::HeaderMap,
    config: &egpu_manager_common::config::Config,
) -> Result<(), (StatusCode, Json<ApiError>)> {
    let token = &config.local_api.api_token;
    if token.is_empty() {
        return Ok(()); // Keine Auth konfiguriert, abwaertskompatibel
    }

    let auth_header = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok());

    match auth_header {
        Some(value) if value.starts_with("Bearer ") => {
            let provided = &value[7..];
            if provided == token {
                Ok(())
            } else {
                Err(api_error(StatusCode::FORBIDDEN, "Ungueltiger API-Token"))
            }
        }
        _ => Err(api_error(StatusCode::UNAUTHORIZED, "Bearer-Token erforderlich")),
    }
}

// ─── GET /api/telemetry/:pci_address ──────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct TelemetryQuery {
    pub hours: Option<u32>,
}

/// GET /api/telemetry/:pci_address — GPU-Telemetrie der letzten N Stunden
pub async fn get_telemetry(
    State(state): State<Arc<AppState>>,
    Path(pci_address): Path<String>,
    Query(query): Query<TelemetryQuery>,
) -> impl IntoResponse {
    let hours = query.hours.unwrap_or(24).min(168); // max 7 Tage

    match state.db.query_telemetry(&pci_address, hours).await {
        Ok(data) => Json(serde_json::json!({
            "pci_address": pci_address,
            "hours": hours,
            "count": data.len(),
            "data": data,
        }))
        .into_response(),
        Err(e) => api_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Telemetrie nicht lesbar: {e}"),
        )
        .into_response(),
    }
}

// ─── GET /api/v1/discover — API-Discovery fuer externe Apps ────────────────

/// Discovery-Endpoint: Beschreibt alle verfuegbaren API-Endpunkte,
/// Authentifizierung, GPU-UUIDs und LLM-Gateway-Integration.
/// Andere Anwendungen koennen diesen Endpoint nutzen, um sich automatisch
/// an den eGPU Manager anzubinden.
pub async fn get_discover(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let cfg = state.config.load();
    let monitor_state = state.monitor_state.lock().await;

    let gpus: Vec<serde_json::Value> = monitor_state
        .gpu_status
        .iter()
        .map(|g| {
            let gpu_type = if g.pci_address == cfg.gpu.egpu_pci_address {
                "egpu"
            } else {
                "internal"
            };
            serde_json::json!({
                "pci_address": g.pci_address,
                "gpu_uuid": g.gpu_uuid,
                "name": g.name,
                "type": gpu_type,
                "memory_total_mb": g.memory_total_mb,
                "nvidia_index": g.nvidia_index,
            })
        })
        .collect();

    let port = cfg.local_api.port;
    let llm_gateway_active = state.llm_router.is_some();

    Json(serde_json::json!({
        "service": "egpu-manager",
        "version": env!("CARGO_PKG_VERSION"),
        "description": "eGPU Manager — GPU Monitoring, Scheduling, LLM Gateway",
        "base_url": {
            "host": format!("http://localhost:{port}"),
            "docker": format!("http://host.docker.internal:{port}"),
            "note": "Docker-Container muessen host.docker.internal nutzen (--add-host=host.docker.internal:host-gateway)"
        },
        "auth": {
            "type": if cfg.local_api.api_token.is_empty() { "none" } else { "bearer" },
            "header": "Authorization",
            "note": "Bearer-Token nur fuer destruktive Endpunkte (POST). GET ist frei."
        },
        "llm_gateway": {
            "active": llm_gateway_active,
            "note": if llm_gateway_active {
                "LLM Gateway ist aktiv und nimmt Requests an"
            } else {
                "[llm_gateway] Section in /etc/egpu-manager/config.toml fehlt oder enabled=false"
            }
        },
        "gpus": gpus,
        "egpu_pci_address": cfg.gpu.egpu_pci_address,
        "internal_pci_address": cfg.gpu.internal_pci_address,
        "recommended_workflow": {
            "step_1": "GET /api/v1/discover — API kennenlernen, GPUs + UUIDs abrufen",
            "step_2": "POST /api/gpu/acquire — Lease anfordern (liefert gpu_uuid + nvidia_index)",
            "step_3": "Im Worker: cuda:{nvidia_index} als Device nutzen",
            "step_4": "POST /api/gpu/release — Lease freigeben wenn fertig",
            "note": "NVIDIA_VISIBLE_DEVICES ist ein Startzeit-Mechanismus. Fuer Laufzeit-Switching: Lease-basiertes Device-Mapping ueber nvidia_index verwenden."
        },
        "endpoints": {
            "status": {
                "method": "GET",
                "path": "/api/status",
                "description": "GPU-Status, Health Score, Warning Level, Pipelines"
            },
            "gpu_acquire": {
                "method": "POST",
                "path": "/api/gpu/acquire",
                "description": "GPU-Lease anfordern (VRAM reservieren)",
                "body": {"pipeline": "string", "workload_type": "string", "vram_mb": 4000, "duration_seconds": 3600},
                "returns": {"granted": true, "gpu_device": "PCI", "gpu_uuid": "GPU-xxx", "lease_id": "uuid"}
            },
            "gpu_release": {
                "method": "POST",
                "path": "/api/gpu/release",
                "body": {"lease_id": "uuid"}
            },
            "gpu_recommend": {
                "method": "GET",
                "path": "/api/gpu/recommend",
                "description": "Empfohlene GPU basierend auf Last und Warning Level"
            },
            "llm_chat": {
                "method": "POST",
                "path": "/api/llm/chat/completions",
                "description": "OpenAI-kompatible Chat Completion via LLM Gateway",
                "headers": {"X-App-Id": "app_name"},
                "body": {"model": "string", "messages": [{"role": "user", "content": "string"}]},
                "compatible_with": "OpenAI Chat Completions API"
            },
            "llm_providers": {
                "method": "GET",
                "path": "/api/llm/providers",
                "description": "Verfuegbare LLM-Provider und deren Status"
            },
            "llm_health": {
                "method": "GET",
                "path": "/api/llm/health",
                "description": "Health-Check des LLM Gateways"
            },
            "telemetry": {
                "method": "GET",
                "path": "/api/telemetry/{pci_address}?hours=24",
                "description": "GPU-Telemetrie-Historie (Temp, VRAM, Power, Health Score)"
            },
            "events_stream": {
                "method": "GET",
                "path": "/api/events/stream",
                "description": "Server-Sent Events (SSE) fuer Echtzeit-Updates",
                "event_types": ["gpu_status", "warning_level", "health_score", "recovery_stage", "pipeline_change"]
            }
        },
        "integration": {
            "python_client": "pip install egpu-llm-client (oder clients/python/egpu_llm_client.py kopieren)",
            "docker_compose": {
                "extra_hosts": "host.docker.internal:host-gateway",
                "environment": format!("EGPU_MANAGER_URL=http://host.docker.internal:{port}"),
                "note": "In docker-compose.yml: extra_hosts und EGPU_MANAGER_URL setzen"
            },
            "env_vars": {
                "EGPU_MANAGER_URL": {
                    "host": format!("http://localhost:{port}"),
                    "docker": format!("http://host.docker.internal:{port}")
                },
                "CUDA_DEVICE": "nvidia_index aus /api/gpu/acquire (z.B. cuda:1 fuer eGPU)"
            }
        }
    }))
}
