//! Remote GPU Listener (Port 7843)
//!
//! Separater Axum-HTTP-Server fuer Remote-GPU-Knoten (z.B. Windows-Maschinen
//! mit NVIDIA-GPUs), die sich per LAN registrieren.
//! Token-basierte Authentifizierung: jeder Request benoetigt
//! `Authorization: Bearer <token>`.

use std::net::SocketAddr;
use std::sync::Arc;

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use axum::routing::post;
use axum::{Json, Router};
use chrono::Utc;
use egpu_manager_common::config::Config;
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;
use tracing::{error, info, warn};

use crate::db::{EventDb, Severity};
use crate::monitor::{MonitorState, RegisteredRemoteGpu};

// ---- Shared state for the remote listener ----

/// Gemeinsamer Zustand fuer den Remote-Listener.
pub struct RemoteListenerState {
    #[allow(dead_code)]
    pub config: Arc<Config>,
    pub db: EventDb,
    pub monitor_state: Arc<Mutex<MonitorState>>,
    pub token: String,
}

// ---- Request/Response types ----

#[derive(Debug, Deserialize)]
pub struct RegisterRequest {
    pub name: String,
    pub host: String,
    #[serde(default = "default_ollama_port")]
    pub port_ollama: u16,
    #[serde(default = "default_agent_port")]
    pub port_agent: u16,
    pub gpu_name: String,
    pub vram_mb: u64,
}

fn default_ollama_port() -> u16 {
    11434
}
fn default_agent_port() -> u16 {
    7843
}

#[derive(Debug, Deserialize)]
pub struct UnregisterRequest {
    pub name: String,
}

#[derive(Debug, Deserialize)]
pub struct HeartbeatRequest {
    pub name: String,
    #[serde(default)]
    pub latency_ms: Option<u32>,
    #[serde(default)]
    pub gpu_name: Option<String>,
    #[serde(default)]
    pub vram_mb: Option<u64>,
}

#[derive(Debug, Serialize)]
struct ApiError {
    error: String,
    code: u16,
}

#[derive(Debug, Serialize)]
struct OkResponse {
    ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    message: Option<String>,
}

// ---- Token-Authentifizierung ----

/// Prueft den Bearer-Token aus dem Authorization-Header.
fn check_auth(headers: &HeaderMap, expected_token: &str) -> Result<(), (StatusCode, Json<ApiError>)> {
    let auth_header = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");

    let token = auth_header.strip_prefix("Bearer ").unwrap_or("");

    if token.is_empty() || token != expected_token {
        return Err((
            StatusCode::UNAUTHORIZED,
            Json(ApiError {
                error: "Ungueltiger oder fehlender Token".to_string(),
                code: 401,
            }),
        ));
    }

    Ok(())
}

// ---- Endpoints ----

/// POST /api/remote/register — Remote-GPU-Knoten registrieren
async fn register(
    State(state): State<Arc<RemoteListenerState>>,
    headers: HeaderMap,
    Json(body): Json<RegisterRequest>,
) -> impl IntoResponse {
    if let Err(e) = check_auth(&headers, &state.token) {
        return e.into_response();
    }

    let now = Utc::now();
    let mut monitor = state.monitor_state.lock().await;

    // Vorhandenen Eintrag aktualisieren oder neuen erstellen
    if let Some(existing) = monitor.remote_gpus.iter_mut().find(|g| g.name == body.name) {
        existing.host = body.host.clone();
        existing.port_ollama = body.port_ollama;
        existing.port_agent = body.port_agent;
        existing.gpu_name = body.gpu_name.clone();
        existing.vram_mb = body.vram_mb;
        existing.status = "online".to_string();
        existing.last_heartbeat = now;
        info!(
            "Remote-GPU aktualisiert: {} ({})",
            body.name, body.gpu_name
        );
    } else {
        monitor.remote_gpus.push(RegisteredRemoteGpu {
            name: body.name.clone(),
            host: body.host.clone(),
            port_ollama: body.port_ollama,
            port_agent: body.port_agent,
            gpu_name: body.gpu_name.clone(),
            vram_mb: body.vram_mb,
            status: "online".to_string(),
            last_heartbeat: now,
            latency_ms: None,
        });
        info!(
            "Remote-GPU registriert: {} ({}, {} MB VRAM)",
            body.name, body.gpu_name, body.vram_mb
        );
    }
    drop(monitor);

    // Event in Datenbank protokollieren
    state
        .db
        .log_event(
            "remote.register",
            Severity::Info,
            &format!(
                "Remote-GPU '{}' registriert: {} ({} MB VRAM) auf {}",
                body.name, body.gpu_name, body.vram_mb, body.host
            ),
            Some(serde_json::json!({
                "name": body.name,
                "host": body.host,
                "gpu_name": body.gpu_name,
                "vram_mb": body.vram_mb,
                "port_ollama": body.port_ollama,
                "port_agent": body.port_agent,
            })),
        )
        .await
        .ok();

    Json(OkResponse {
        ok: true,
        message: Some(format!("Remote-GPU '{}' registriert", body.name)),
    })
    .into_response()
}

/// POST /api/remote/unregister — Remote-GPU-Knoten abmelden
async fn unregister(
    State(state): State<Arc<RemoteListenerState>>,
    headers: HeaderMap,
    Json(body): Json<UnregisterRequest>,
) -> impl IntoResponse {
    if let Err(e) = check_auth(&headers, &state.token) {
        return e.into_response();
    }

    let mut monitor = state.monitor_state.lock().await;
    let before_len = monitor.remote_gpus.len();
    monitor.remote_gpus.retain(|g| g.name != body.name);
    let removed = monitor.remote_gpus.len() < before_len;
    drop(monitor);

    if removed {
        info!("Remote-GPU abgemeldet: {}", body.name);

        state
            .db
            .log_event(
                "remote.unregister",
                Severity::Info,
                &format!("Remote-GPU '{}' abgemeldet", body.name),
                Some(serde_json::json!({
                    "name": body.name,
                })),
            )
            .await
            .ok();

        Json(OkResponse {
            ok: true,
            message: Some(format!("Remote-GPU '{}' abgemeldet", body.name)),
        })
        .into_response()
    } else {
        (
            StatusCode::NOT_FOUND,
            Json(ApiError {
                error: format!("Remote-GPU '{}' nicht gefunden", body.name),
                code: 404,
            }),
        )
            .into_response()
    }
}

/// POST /api/remote/heartbeat — Periodischer Heartbeat von Remote-Knoten
async fn heartbeat(
    State(state): State<Arc<RemoteListenerState>>,
    headers: HeaderMap,
    Json(body): Json<HeartbeatRequest>,
) -> impl IntoResponse {
    if let Err(e) = check_auth(&headers, &state.token) {
        return e.into_response();
    }

    let now = Utc::now();
    let mut monitor = state.monitor_state.lock().await;

    if let Some(existing) = monitor.remote_gpus.iter_mut().find(|g| g.name == body.name) {
        existing.last_heartbeat = now;
        existing.status = "online".to_string();
        existing.latency_ms = body.latency_ms;

        // Optionale Aktualisierung von GPU-Metadaten
        if let Some(ref gpu_name) = body.gpu_name {
            existing.gpu_name = gpu_name.clone();
        }
        if let Some(vram_mb) = body.vram_mb {
            existing.vram_mb = vram_mb;
        }

        Json(OkResponse {
            ok: true,
            message: None,
        })
        .into_response()
    } else {
        (
            StatusCode::NOT_FOUND,
            Json(ApiError {
                error: format!(
                    "Remote-GPU '{}' nicht registriert — bitte zuerst /api/remote/register aufrufen",
                    body.name
                ),
                code: 404,
            }),
        )
            .into_response()
    }
}

// ---- Router und Server ----

fn build_remote_router(state: Arc<RemoteListenerState>) -> Router {
    Router::new()
        .route("/api/remote/register", post(register))
        .route("/api/remote/unregister", post(unregister))
        .route("/api/remote/heartbeat", post(heartbeat))
        .with_state(state)
}

/// Token aus Datei lesen oder generieren.
/// Wenn die Datei nicht existiert, wird ein zufaelliger Token erzeugt und
/// in die Datei geschrieben.
fn load_or_generate_token(token_path: &str) -> anyhow::Result<String> {
    if token_path.is_empty() {
        // Kein Pfad angegeben — zufaelligen Token generieren (nur im Speicher)
        let token = generate_random_token();
        warn!(
            "Kein remote.token_path konfiguriert — generierter Token (nur diese Sitzung): {}",
            token
        );
        return Ok(token);
    }

    let path = std::path::Path::new(token_path);

    if path.exists() {
        let content = std::fs::read_to_string(path)
            .map_err(|e| anyhow::anyhow!("Token-Datei nicht lesbar: {path:?}: {e}"))?;
        let token = content.trim().to_string();
        if token.is_empty() {
            anyhow::bail!("Token-Datei ist leer: {path:?}");
        }
        info!("Remote-Token geladen aus {}", token_path);
        Ok(token)
    } else {
        // Token generieren und in Datei schreiben
        let token = generate_random_token();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).ok();
        }
        std::fs::write(path, &token)
            .map_err(|e| anyhow::anyhow!("Token-Datei nicht schreibbar: {path:?}: {e}"))?;
        info!(
            "Remote-Token generiert und gespeichert in {}",
            token_path
        );
        Ok(token)
    }
}

/// Erzeugt einen zufaelligen 32-Byte-Hex-Token.
fn generate_random_token() -> String {
    use rand::Rng;
    let mut rng = rand::rng();
    let bytes: [u8; 32] = rng.random();
    hex::encode(bytes)
}

/// Startet den Remote-Listener-Server.
/// Bindet standardmaessig auf 0.0.0.0:7843 (konfigurierbar).
pub async fn start_remote_listener(
    config: Arc<Config>,
    db: EventDb,
    monitor_state: Arc<Mutex<MonitorState>>,
    cancel: CancellationToken,
) {
    let remote_cfg = match &config.remote {
        Some(cfg) if cfg.enabled => cfg.clone(),
        Some(_) => {
            info!("Remote-Listener ist deaktiviert (remote.enabled = false)");
            return;
        }
        None => {
            info!("Remote-Listener nicht konfiguriert ([remote] fehlt in config)");
            return;
        }
    };

    // Token laden oder generieren
    let token = match load_or_generate_token(&remote_cfg.token_path) {
        Ok(t) => t,
        Err(e) => {
            error!("Remote-Listener Token-Fehler: {e}");
            return;
        }
    };

    let state = Arc::new(RemoteListenerState {
        config,
        db,
        monitor_state,
        token,
    });

    let router = build_remote_router(state);

    let addr = SocketAddr::new(
        remote_cfg
            .bind
            .parse()
            .unwrap_or_else(|_| std::net::IpAddr::V4(std::net::Ipv4Addr::UNSPECIFIED)),
        remote_cfg.port,
    );

    info!("Remote-Listener startet auf http://{addr}");

    let listener = match tokio::net::TcpListener::bind(addr).await {
        Ok(l) => l,
        Err(e) => {
            error!("Remote-Listener konnte nicht starten: {e}");
            return;
        }
    };

    axum::serve(listener, router)
        .with_graceful_shutdown(async move {
            cancel.cancelled().await;
            info!("Remote-Listener wird beendet");
        })
        .await
        .unwrap_or_else(|e| error!("Remote-Listener Fehler: {e}"));
}

/// Hintergrund-Task: markiert Remote-GPUs als "stale" oder "offline",
/// wenn kein Heartbeat mehr empfangen wird.
/// Aufgerufen aus dem Monitor-Orchestrator oder als eigener Task.
pub async fn remote_gpu_staleness_loop(
    monitor_state: Arc<Mutex<MonitorState>>,
    cancel: CancellationToken,
) {
    let interval = std::time::Duration::from_secs(15);
    let stale_threshold = chrono::Duration::seconds(60);
    let offline_threshold = chrono::Duration::seconds(180);

    loop {
        tokio::select! {
            _ = cancel.cancelled() => {
                info!("Remote-GPU-Staleness-Task beendet");
                return;
            }
            _ = tokio::time::sleep(interval) => {
                let now = Utc::now();
                let mut monitor = monitor_state.lock().await;
                for gpu in &mut monitor.remote_gpus {
                    let age = now - gpu.last_heartbeat;
                    if age > offline_threshold {
                        if gpu.status != "offline" {
                            warn!("Remote-GPU '{}' ist offline (kein Heartbeat seit {} Sek.)", gpu.name, age.num_seconds());
                            gpu.status = "offline".to_string();
                        }
                    } else if age > stale_threshold {
                        if gpu.status != "stale" {
                            warn!("Remote-GPU '{}' ist stale (kein Heartbeat seit {} Sek.)", gpu.name, age.num_seconds());
                            gpu.status = "stale".to_string();
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scheduler::{GpuCapacity, VramScheduler};
    use crate::warning::WarningStateMachine;
    use axum::body::Body;
    use axum::http::Request;
    use std::collections::HashMap;
    use tower::ServiceExt;

    fn make_test_state(token: &str) -> Arc<RemoteListenerState> {
        let config_str = r#"
            schema_version = 1
            [gpu]
            egpu_pci_address = "0000:05:00.0"
            internal_pci_address = "0000:02:00.0"
        "#;
        let config: Config = toml::from_str(config_str).unwrap();

        let db = EventDb::open_in_memory().unwrap();
        let warning_machine = WarningStateMachine::new(120);
        let scheduler = VramScheduler::new(
            GpuCapacity {
                total_vram_mb: 16000,
                display_reserve_mb: 0,
            },
            GpuCapacity {
                total_vram_mb: 8000,
                display_reserve_mb: 512,
            },
            90,
        );
        let health_score = crate::health_score::LinkHealthScore::new(
            3.0, 5.0, 2.0, 5.0, 1.0, 60.0, 40.0,
        );
        let monitor_state = Arc::new(Mutex::new(MonitorState {
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

        Arc::new(RemoteListenerState {
            config: Arc::new(config),
            db,
            monitor_state,
            token: token.to_string(),
        })
    }

    #[tokio::test]
    async fn test_register_without_auth() {
        let state = make_test_state("test-token");
        let app = build_remote_router(state);

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/remote/register")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        r#"{"name":"win-pc","host":"192.168.1.100","gpu_name":"RTX 4090","vram_mb":24576}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn test_register_with_wrong_token() {
        let state = make_test_state("test-token");
        let app = build_remote_router(state);

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/remote/register")
                    .header("content-type", "application/json")
                    .header("authorization", "Bearer wrong-token")
                    .body(Body::from(
                        r#"{"name":"win-pc","host":"192.168.1.100","gpu_name":"RTX 4090","vram_mb":24576}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn test_register_success() {
        let state = make_test_state("test-token");
        let monitor_state = Arc::clone(&state.monitor_state);
        let app = build_remote_router(state);

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/remote/register")
                    .header("content-type", "application/json")
                    .header("authorization", "Bearer test-token")
                    .body(Body::from(
                        r#"{"name":"win-pc","host":"192.168.1.100","gpu_name":"RTX 4090","vram_mb":24576}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), 1024 * 1024)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["ok"], true);

        // Pruefen, dass die GPU im MonitorState registriert ist
        let monitor = monitor_state.lock().await;
        assert_eq!(monitor.remote_gpus.len(), 1);
        assert_eq!(monitor.remote_gpus[0].name, "win-pc");
        assert_eq!(monitor.remote_gpus[0].gpu_name, "RTX 4090");
        assert_eq!(monitor.remote_gpus[0].vram_mb, 24576);
        assert_eq!(monitor.remote_gpus[0].status, "online");
    }

    #[tokio::test]
    async fn test_register_and_unregister() {
        let state = make_test_state("test-token");
        let monitor_state = Arc::clone(&state.monitor_state);

        // Registrieren
        let app = build_remote_router(Arc::clone(&state));
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/remote/register")
                    .header("content-type", "application/json")
                    .header("authorization", "Bearer test-token")
                    .body(Body::from(
                        r#"{"name":"win-pc","host":"192.168.1.100","gpu_name":"RTX 4090","vram_mb":24576}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        // Abmelden
        let app2 = build_remote_router(state);
        let response = app2
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/remote/unregister")
                    .header("content-type", "application/json")
                    .header("authorization", "Bearer test-token")
                    .body(Body::from(r#"{"name":"win-pc"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let monitor = monitor_state.lock().await;
        assert!(monitor.remote_gpus.is_empty());
    }

    #[tokio::test]
    async fn test_heartbeat_unregistered() {
        let state = make_test_state("test-token");
        let app = build_remote_router(state);

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/remote/heartbeat")
                    .header("content-type", "application/json")
                    .header("authorization", "Bearer test-token")
                    .body(Body::from(
                        r#"{"name":"unknown-pc","latency_ms":5}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn test_register_then_heartbeat() {
        let state = make_test_state("test-token");
        let monitor_state = Arc::clone(&state.monitor_state);

        // Registrieren
        let app = build_remote_router(Arc::clone(&state));
        let _ = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/remote/register")
                    .header("content-type", "application/json")
                    .header("authorization", "Bearer test-token")
                    .body(Body::from(
                        r#"{"name":"win-pc","host":"192.168.1.100","gpu_name":"RTX 4090","vram_mb":24576}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();

        // Heartbeat mit Latenz
        let app2 = build_remote_router(state);
        let response = app2
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/remote/heartbeat")
                    .header("content-type", "application/json")
                    .header("authorization", "Bearer test-token")
                    .body(Body::from(
                        r#"{"name":"win-pc","latency_ms":3}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);

        let monitor = monitor_state.lock().await;
        assert_eq!(monitor.remote_gpus[0].latency_ms, Some(3));
    }

    #[test]
    fn test_generate_random_token() {
        let token = generate_random_token();
        assert_eq!(token.len(), 64); // 32 bytes = 64 hex chars
    }

    #[tokio::test]
    async fn test_unregister_nonexistent() {
        let state = make_test_state("test-token");
        let app = build_remote_router(state);

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/remote/unregister")
                    .header("content-type", "application/json")
                    .header("authorization", "Bearer test-token")
                    .body(Body::from(r#"{"name":"nonexistent"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }
}
