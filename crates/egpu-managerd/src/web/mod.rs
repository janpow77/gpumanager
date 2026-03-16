pub mod api;
pub mod sse;
pub mod ui;

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Instant;

use axum::http::{header, HeaderValue, Method, StatusCode};
use axum::response::{Html, IntoResponse};
use axum::routing::{get, post, put};
use axum::Router;
use egpu_manager_common::config::Config;
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;
use tower_http::cors::CorsLayer;
use tracing::{error, info};

use crate::db::EventDb;
use crate::monitor::MonitorState;
use crate::web::sse::SseBroadcaster;

/// Shared application state for all Axum handlers.
pub struct AppState {
    pub config: Arc<Config>,
    pub db: EventDb,
    pub monitor_state: Arc<Mutex<MonitorState>>,
    pub sse: SseBroadcaster,
    pub started_at: Instant,
}

/// Serve the embedded HTML UI at root.
async fn serve_index() -> impl IntoResponse {
    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
        Html(ui::INDEX_HTML),
    )
}

/// Build the Axum router with all API routes.
fn build_router(state: Arc<AppState>) -> Router {
    // CORS configuration from config
    let cors = {
        let mut cors = CorsLayer::new()
            .allow_methods([
                Method::GET,
                Method::POST,
                Method::PUT,
                Method::DELETE,
                Method::OPTIONS,
            ])
            .allow_headers([
                header::CONTENT_TYPE,
                header::ACCEPT,
                header::AUTHORIZATION,
            ]);

        if state.config.local_api.cors_origins.is_empty() {
            cors = cors.allow_origin(tower_http::cors::Any);
        } else {
            let origins: Vec<HeaderValue> = state
                .config
                .local_api
                .cors_origins
                .iter()
                .filter_map(|o| o.parse().ok())
                .collect();
            cors = cors.allow_origin(origins);
        }
        cors
    };

    Router::new()
        // Embedded UI
        .route("/", get(serve_index))
        // Status
        .route("/api/status", get(api::get_status))
        // Pipelines
        .route("/api/pipelines", get(api::get_pipelines))
        .route("/api/pipelines/{container}", get(api::get_pipeline))
        .route(
            "/api/pipelines/{container}/priority",
            put(api::put_pipeline_priority),
        )
        .route(
            "/api/pipelines/{container}/assign",
            post(api::post_pipeline_assign),
        )
        .route(
            "/api/pipelines/{container}/workload-update",
            post(api::post_workload_update),
        )
        // GPU acquire/release/recommend (Gap 4)
        .route("/api/gpu/acquire", post(api::post_gpu_acquire))
        .route("/api/gpu/release", post(api::post_gpu_release))
        .route("/api/gpu/recommend", get(api::get_gpu_recommend))
        // eGPU admission control
        .route("/api/egpu/admission", post(api::post_egpu_admission))
        // Ollama
        .route("/api/ollama/status", get(api::get_ollama_status))
        .route("/api/ollama/unload", post(api::post_ollama_unload))
        // Recovery
        .route("/api/recovery/status", get(api::get_recovery_status))
        .route("/api/recovery/reset", post(api::post_recovery_reset))
        .route(
            "/api/recovery/thunderbolt-reconnect",
            post(api::post_thunderbolt_reconnect),
        )
        // Events / SSE
        .route("/api/events", get(api::get_events))
        .route("/api/events/stream", get(api::get_events_stream))
        // Audit
        .route("/api/audit-log", get(api::get_audit_log))
        // Config
        .route("/api/config", get(api::get_config))
        .route("/api/config/reload", post(api::post_config_reload))
        // System stats
        .route("/api/system", get(api::get_system_stats))
        // Setup generator (Windows Remote-Node)
        .route("/api/setup/generate", post(api::post_setup_generate))
        .route("/api/setup/instructions", get(api::get_setup_instructions))
        // Middleware
        .layer(cors)
        .with_state(state)
}

/// Start the web server. Binds to 127.0.0.1:7842 (hardcoded).
/// Returns when the cancellation token is triggered.
/// Returns the SseBroadcaster so it can be shared with the monitor.
pub fn create_sse_broadcaster() -> SseBroadcaster {
    SseBroadcaster::new(256)
}

pub async fn start_web_server(
    config: Arc<Config>,
    db: EventDb,
    monitor_state: Arc<Mutex<MonitorState>>,
    cancel: CancellationToken,
    sse: SseBroadcaster,
) {
    let state = Arc::new(AppState {
        config,
        db,
        monitor_state,
        sse,
        started_at: Instant::now(),
    });

    let router = build_router(state);

    let addr = SocketAddr::from(([127, 0, 0, 1], 7842));
    info!("Web-Server startet auf http://{addr}");

    let listener = match tokio::net::TcpListener::bind(addr).await {
        Ok(l) => l,
        Err(e) => {
            error!("Web-Server konnte nicht starten: {e}");
            return;
        }
    };

    axum::serve(listener, router)
        .with_graceful_shutdown(async move {
            cancel.cancelled().await;
            info!("Web-Server wird beendet");
        })
        .await
        .unwrap_or_else(|e| error!("Web-Server Fehler: {e}"));
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

    fn make_test_state() -> Arc<AppState> {
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

        Arc::new(AppState {
            config: Arc::new(config),
            db,
            monitor_state,
            sse: SseBroadcaster::new(64),
            started_at: Instant::now(),
        })
    }

    #[tokio::test]
    async fn test_index_returns_html() {
        let state = make_test_state();
        let app = build_router(state);

        let response = app
            .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let ct = response.headers().get("content-type").unwrap();
        assert!(ct.to_str().unwrap().contains("text/html"));
    }

    #[tokio::test]
    async fn test_get_status() {
        let state = make_test_state();
        let app = build_router(state);

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/status")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), 1024 * 1024)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert!(json.get("daemon").is_some());
        assert!(json.get("gpus").is_some());
        assert!(json.get("remote_gpus").is_some());
    }

    #[tokio::test]
    async fn test_get_pipelines_empty() {
        let state = make_test_state();
        let app = build_router(state);

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/pipelines")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), 1024 * 1024)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert!(json.as_array().unwrap().is_empty());
    }

    #[tokio::test]
    async fn test_get_events() {
        let state = make_test_state();
        let app = build_router(state);

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/events?limit=10")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_get_recovery_status() {
        let state = make_test_state();
        let app = build_router(state);

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/recovery/status")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), 1024 * 1024)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["active"], false);
    }

    #[tokio::test]
    async fn test_get_config() {
        let state = make_test_state();
        let app = build_router(state);

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/config")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_pipeline_not_found() {
        let state = make_test_state();
        let app = build_router(state);

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/pipelines/nonexistent")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn test_recovery_reset_requires_confirm() {
        let state = make_test_state();
        let app = build_router(state);

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/recovery/reset")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"confirm":false}"#))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn test_dry_run_config_reload() {
        let state = make_test_state();
        let app = build_router(state);

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/config/reload?dry_run=true")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), 1024 * 1024)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["dry_run"], true);
    }

    #[tokio::test]
    async fn test_gpu_acquire_and_release() {
        let state = make_test_state();
        let app = build_router(Arc::clone(&state));

        // Acquire
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/gpu/acquire")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        r#"{"pipeline":"test","workload_type":"ocr","vram_mb":4000}"#,
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
        assert_eq!(json["granted"], true);
        let lease_id = json["lease_id"].as_str().unwrap().to_string();

        // Release
        let app2 = build_router(state);
        let response = app2
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/gpu/release")
                    .header("content-type", "application/json")
                    .body(Body::from(format!(r#"{{"lease_id":"{lease_id}"}}"#)))
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
    }

    #[tokio::test]
    async fn test_gpu_recommend() {
        let state = make_test_state();
        let app = build_router(state);

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/gpu/recommend")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), 1024 * 1024)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert!(json.get("recommended_gpu").is_some());
        assert!(json.get("warning_level").is_some());
    }
}
