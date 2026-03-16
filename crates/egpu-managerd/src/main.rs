mod aer;
#[allow(dead_code)]
mod db;
mod docker;
mod health_score;
mod kmsg;
mod link_health;
mod llm;
mod monitor;
mod nvidia;
#[allow(dead_code)]
mod ollama;
#[allow(dead_code)]
mod recovery;
mod remote_listener;
mod scheduler;
mod setup_generator;
mod sysinfo;
mod web;
mod sysfs;
mod warning;

use std::path::PathBuf;

use clap::Parser;
use egpu_manager_common::config::Config;
use egpu_manager_common::hal::{AerMonitor, PcieLinkMonitor};
use tokio_util::sync::CancellationToken;
use tracing::{error, info, warn};

use crate::db::EventDb;
use crate::docker::DockerComposeControl;
use crate::monitor::MonitorOrchestrator;
use crate::recovery::RecoveryStateMachine;

#[derive(Parser)]
#[command(name = "egpu-managerd", about = "eGPU Manager Daemon")]
struct Cli {
    /// Pfad zur Konfigurationsdatei
    #[arg(short, long, default_value = "/etc/egpu-manager/config.toml")]
    config: PathBuf,

    /// Nur GPU-Status ausgeben und beenden
    #[arg(long)]
    status: bool,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info".into()),
        )
        .init();

    let cli = Cli::parse();

    info!("egpu-managerd v{} wird gestartet", env!("CARGO_PKG_VERSION"));

    // Konfiguration laden
    let config = Config::load(&cli.config)?;
    info!(
        "Konfiguration geladen (Schema v{}): eGPU={}, intern={}",
        config.schema_version, config.gpu.egpu_pci_address, config.gpu.internal_pci_address
    );
    info!(
        "{} Pipeline(s) konfiguriert",
        config.pipeline.len()
    );

    // GPU-Status abfragen
    let gpu_monitor = nvidia::NvidiaSmiMonitor::new(config.gpu.nvidia_smi_timeout_seconds);

    match gpu_monitor.query_all().await {
        Ok(gpus) => {
            for gpu in &gpus {
                let gpu_type = if gpu.pci_address == config.gpu.egpu_pci_address {
                    "eGPU"
                } else if gpu.pci_address == config.gpu.internal_pci_address {
                    "intern"
                } else {
                    "unbekannt"
                };

                info!(
                    "GPU [{}] {} ({}): {}°C, GPU {}%, VRAM {}/{} MB, {:.1}W, {}",
                    gpu_type,
                    gpu.name,
                    gpu.pci_address,
                    gpu.temperature_c,
                    gpu.utilization_gpu_percent,
                    gpu.memory_used_mb,
                    gpu.memory_total_mb,
                    gpu.power_draw_w,
                    gpu.pstate,
                );
            }

            // PCI-Bus-ID -> Index Mapping
            for gpu in &gpus {
                if let Some(idx) = gpu.nvidia_index {
                    info!(
                        "GPU-Mapping: {} -> nvidia-index {}",
                        gpu.pci_address, idx
                    );
                }
            }
        }
        Err(e) => {
            error!("GPU-Abfrage fehlgeschlagen: {e}");
            warn!("Daemon startet im Degraded Mode");
        }
    }

    // PCIe-Link-Health pruefen
    let link_monitor = sysfs::SysfsLinkMonitor;
    match link_monitor
        .read_link_health(&config.gpu.egpu_pci_address)
        .await
    {
        Ok(health) => {
            let tb_expected_width: u8 = 4;
            let is_tb_degraded = health.current_link_width < tb_expected_width;
            let is_link_down = health.is_link_down();

            info!(
                "PCIe-Link eGPU: {} x{} (Thunderbolt-Erwartung: x{}), {}",
                health.current_link_speed,
                health.current_link_width,
                tb_expected_width,
                if is_link_down {
                    "LINK DOWN"
                } else if is_tb_degraded {
                    "DEGRADIERT (unter Thunderbolt-Erwartung)"
                } else {
                    "OK"
                },
            );
        }
        Err(e) => {
            warn!("PCIe-Link-Health nicht lesbar: {e}");
        }
    }

    // AER-Zaehler pruefen
    let aer_monitor = sysfs::SysfsAerMonitor;
    match aer_monitor
        .read_nonfatal_count(&config.gpu.egpu_pci_address)
        .await
    {
        Ok(count) => {
            info!("AER Non-Fatal-Zaehler eGPU: {count}");
        }
        Err(e) => {
            warn!("AER-Zaehler nicht lesbar: {e}");
        }
    }

    // PCIe-Bandbreite (nvidia-smi dmon)
    match gpu_monitor
        .query_pcie_throughput(&config.gpu.egpu_pci_address)
        .await
    {
        Ok(throughput) => {
            let max_throughput_kbps: u64 = 1_000_000;
            let utilization =
                (throughput.tx_kbps + throughput.rx_kbps) as f64 / max_throughput_kbps as f64
                    * 100.0;
            info!(
                "PCIe-Bandbreite eGPU: TX={} KB/s, RX={} KB/s ({:.1}% Auslastung)",
                throughput.tx_kbps, throughput.rx_kbps, utilization
            );
        }
        Err(e) => {
            warn!("PCIe-Bandbreite nicht messbar: {e}");
        }
    }

    // Display-VRAM-Reservierung
    match gpu_monitor
        .query_display_vram(&config.gpu.internal_pci_address)
        .await
    {
        Ok(display_vram_mb) => {
            info!(
                "Display-VRAM-Reservierung auf interner GPU: {} MB",
                display_vram_mb
            );
        }
        Err(e) => {
            warn!(
                "Display-VRAM nicht ermittelbar, verwende Fallback: {} MB — {e}",
                config.gpu.display_vram_reserve_mb
            );
        }
    }

    // Ollama-Status
    if let Some(ref ollama_config) = config.ollama
        && ollama_config.enabled
    {
        match nvidia::query_ollama_models(&ollama_config.host).await {
            Ok(models) => {
                if models.is_empty() {
                    info!("Ollama: Keine Modelle geladen");
                } else {
                    for m in &models {
                        info!(
                            "Ollama-Modell: {} ({:.1} GB VRAM)",
                            m.name,
                            m.size_vram_bytes as f64 / 1024.0 / 1024.0 / 1024.0
                        );
                    }
                }
            }
            Err(e) => {
                warn!("Ollama nicht erreichbar: {e}");
            }
        }
    }

    // Pipeline-Status ausgeben
    for p in &config.pipeline {
        info!(
            "Pipeline: {} / {} — Prio {}, GPU {}, VRAM ~{} MB, Workloads: {:?}",
            p.project, p.container, p.gpu_priority, p.gpu_device, p.vram_estimate_mb,
            p.workload_types
        );
    }

    if cli.status {
        info!("Status-Modus — Daemon wird nicht gestartet");
        return Ok(());
    }

    // --- Daemon mode: start monitoring ---

    // Open event database
    let db_path = std::path::Path::new(&config.database.db_path);
    let db = EventDb::open(db_path)?;
    info!("Event-Datenbank geoeffnet: {}", config.database.db_path);

    db.log_event(
        "daemon.start",
        db::Severity::Info,
        &format!("egpu-managerd v{} gestartet", env!("CARGO_PKG_VERSION")),
        None,
    )
    .await?;

    // Check for interrupted recovery — always discard on daemon restart.
    // A daemon restart (systemd, manual) is itself a reset. If nvidia-smi works
    // after restart, there's no reason to resume a stale recovery that would
    // re-trigger migrations, PCIe resets etc. and potentially cause more harm.
    {
        let mut recovery_sm = RecoveryStateMachine::new(
            db.clone(),
            config.recovery.reset_cooldown_seconds,
        );
        if let Some(stage) = recovery_sm.check_interrupted().await? {
            warn!(
                "Unterbrochene Recovery '{}' wird verworfen (Daemon wurde neu gestartet)",
                stage
            );
            recovery_sm.clear_interrupted().await.ok();
        }
    }

    // Check for existing fallback override files
    match db.load_fallback_overrides().await {
        Ok(overrides) => {
            let existing = docker::check_existing_overrides(&overrides).await;
            if !existing.is_empty() {
                warn!(
                    "{} Fallback-Override-Dateien aus vorherigem Lauf gefunden",
                    existing.len()
                );
                for ov in &existing {
                    warn!(
                        "  Override aktiv: {} -> {}",
                        ov.service_name, ov.override_path
                    );
                }
            }
        }
        Err(e) => {
            warn!("Fallback-Overrides nicht ladbar: {e}");
        }
    }

    // Set up cancellation for graceful shutdown
    let cancel = CancellationToken::new();
    let cancel_clone = cancel.clone();

    // Handle SIGTERM/SIGINT
    let db_shutdown = db.clone();
    let config_shutdown = config.clone();
    tokio::spawn(async move {
        let mut sigterm =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                .expect("SIGTERM handler");
        let mut sigint =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt())
                .expect("SIGINT handler");

        tokio::select! {
            _ = sigterm.recv() => {
                info!("SIGTERM empfangen — fahre herunter");
            }
            _ = sigint.recv() => {
                info!("SIGINT empfangen — fahre herunter");
            }
        }

        // Persist recovery state if active
        let rsm = RecoveryStateMachine::new(
            db_shutdown,
            config_shutdown.recovery.reset_cooldown_seconds,
        );
        if rsm.is_active()
            && let Err(e) = rsm.mark_interrupted().await
        {
            error!("Recovery-Status nicht persistierbar: {e}");
        }

        cancel_clone.cancel();
    });

    // Create SSE broadcaster to share between web server and monitor
    let sse = web::create_sse_broadcaster();

    // Start monitoring orchestrator
    let mut orchestrator = MonitorOrchestrator::new(config.clone(), db.clone(), cancel.clone());
    orchestrator.set_sse_broadcaster(sse.clone());
    info!("Monitoring-Orchestrator wird gestartet");

    // Start web server in parallel with monitoring
    let web_config = std::sync::Arc::new(config.clone());
    let web_db = db.clone();
    let web_cancel = cancel.clone();
    let monitor_state = orchestrator.state();

    let web_handle = tokio::spawn(async move {
        web::start_web_server(web_config, web_db, monitor_state.clone(), web_cancel.clone(), sse).await;
    });

    // Start remote listener (separate server on port 7843) in parallel
    let remote_config = std::sync::Arc::new(config.clone());
    let remote_db = db.clone();
    let remote_monitor_state = orchestrator.state();
    let remote_cancel = cancel.clone();
    let remote_handle = tokio::spawn(async move {
        remote_listener::start_remote_listener(
            remote_config,
            remote_db,
            remote_monitor_state,
            remote_cancel,
        )
        .await;
    });

    // Start remote GPU staleness checker
    let staleness_monitor_state = orchestrator.state();
    let staleness_cancel = cancel.clone();
    let staleness_handle = tokio::spawn(async move {
        remote_listener::remote_gpu_staleness_loop(
            staleness_monitor_state,
            staleness_cancel,
        )
        .await;
    });

    // Run monitoring (blocks until cancellation)
    orchestrator.run().await;

    // Wait for web server and remote listener to shut down
    let _ = web_handle.await;
    let _ = remote_handle.await;
    let _ = staleness_handle.await;

    // Log shutdown
    db.log_event(
        "daemon.stop",
        db::Severity::Info,
        "egpu-managerd wird beendet",
        None,
    )
    .await
    .ok();

    info!("egpu-managerd beendet");
    Ok(())
}
