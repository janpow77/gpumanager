use clap::Parser;

const DAEMON_URL: &str = "http://127.0.0.1:7842";

#[derive(Parser)]
#[command(name = "egpu-manager", about = "eGPU Manager CLI", version)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(clap::Subcommand)]
enum Commands {
    /// GPU- und Pipeline-Status anzeigen
    Status,

    /// Pipeline-Priorität ändern
    Priority {
        #[command(subcommand)]
        action: PriorityAction,
    },

    /// Konfigurationsmanagement
    Config {
        #[command(subcommand)]
        action: ConfigAction,
    },

    /// Remote-GPU-Verwaltung
    Remote {
        #[command(subcommand)]
        action: RemoteAction,
    },

    /// Projekt-Wizard
    Wizard {
        #[command(subcommand)]
        action: WizardAction,
    },

    /// Weboberfläche im Browser öffnen
    Open,
}

#[derive(clap::Subcommand)]
enum PriorityAction {
    /// Priorität setzen (1=Kritisch, 5=Minimal)
    Set { container: String, priority: u32 },
    /// Aktuelle Priorität anzeigen
    Get { container: String },
}

#[derive(clap::Subcommand)]
enum ConfigAction {
    /// Konfiguration neuladen
    Reload,
    /// Letztes Backup wiederherstellen
    Rollback {
        /// Zeitstempel des Backups (optional, sonst neuestes)
        timestamp: Option<String>,
    },
    /// Alle Backups anzeigen
    ListBackups,
}

#[derive(clap::Subcommand)]
enum RemoteAction {
    /// Remote-Setup initialisieren (Token + optional TLS generieren)
    Init,
    /// Token rotieren
    RotateToken,
    /// Token anzeigen
    ShowToken,
}

#[derive(clap::Subcommand)]
enum WizardAction {
    /// Neues Projekt hinzufügen
    Add { path: String },
    /// Projekt entfernen
    Remove { project: String },
    /// Projekt bearbeiten
    Edit { container: String },
    /// Alle Projekte auflisten
    List,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Status => cmd_status().await?,
        Commands::Priority { action } => match action {
            PriorityAction::Set {
                container,
                priority,
            } => cmd_priority_set(&container, priority).await?,
            PriorityAction::Get { container } => cmd_priority_get(&container).await?,
        },
        Commands::Config { action } => match action {
            ConfigAction::Reload => cmd_config_reload().await?,
            ConfigAction::Rollback { timestamp } => {
                println!(
                    "Config-Rollback: {} (noch nicht implementiert — Phase 6+)",
                    timestamp.as_deref().unwrap_or("neuestes Backup")
                );
            }
            ConfigAction::ListBackups => {
                println!("Backups auflisten (noch nicht implementiert — Phase 6+)");
            }
        },
        Commands::Remote { action } => match action {
            RemoteAction::Init => {
                println!("Remote-Setup initialisieren (noch nicht implementiert — Phase 6a)");
            }
            RemoteAction::RotateToken => {
                println!("Token rotieren (noch nicht implementiert — Phase 6a)");
            }
            RemoteAction::ShowToken => {
                println!("Token anzeigen (noch nicht implementiert — Phase 6a)");
            }
        },
        Commands::Wizard { action } => match action {
            WizardAction::Add { path } => {
                println!("Projekt hinzufügen: {path} (noch nicht implementiert — Phase 4b)");
            }
            WizardAction::Remove { project } => {
                println!("Projekt entfernen: {project} (noch nicht implementiert — Phase 4b)");
            }
            WizardAction::Edit { container } => {
                println!("Projekt bearbeiten: {container} (noch nicht implementiert — Phase 4b)");
            }
            WizardAction::List => cmd_wizard_list().await?,
        },
        Commands::Open => {
            println!("Öffne {DAEMON_URL} im Browser...");
            let _ = std::process::Command::new("xdg-open")
                .arg(DAEMON_URL)
                .spawn();
        }
    }

    Ok(())
}

async fn cmd_status() -> anyhow::Result<()> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .build()?;

    let resp = client
        .get(format!("{DAEMON_URL}/api/status"))
        .send()
        .await;

    match resp {
        Ok(r) if r.status().is_success() => {
            let body: serde_json::Value = r.json().await?;
            print_status(&body);
        }
        Ok(r) => {
            eprintln!("Daemon-Fehler: HTTP {}", r.status());
        }
        Err(_) => {
            eprintln!("Daemon nicht erreichbar auf {DAEMON_URL}");
            eprintln!("Starten mit: sudo systemctl start egpu-manager");
            std::process::exit(1);
        }
    }

    // Pipelines
    let resp = client
        .get(format!("{DAEMON_URL}/api/pipelines"))
        .send()
        .await;

    if let Ok(r) = resp
        && r.status().is_success()
    {
        let body: serde_json::Value = r.json().await?;
        print_pipelines(&body);
    }

    Ok(())
}

fn print_status(status: &serde_json::Value) {
    let daemon = &status["daemon"];
    let warning = daemon["warning_level"].as_str().unwrap_or("unknown");
    let mode = daemon["mode"].as_str().unwrap_or("unknown");
    let uptime = daemon["uptime_seconds"].as_u64().unwrap_or(0);
    let recovery = daemon["recovery_active"].as_bool().unwrap_or(false);

    // Health Score
    let hs_score = status["health_score"]["score"].as_f64().unwrap_or(100.0);
    let hs_events = status["health_score"]["event_count"].as_u64().unwrap_or(0);
    let hs_indicator = if hs_score >= 80.0 {
        "OK"
    } else if hs_score >= 60.0 {
        "WARNUNG"
    } else if hs_score >= 40.0 {
        "NIEDRIG"
    } else {
        "KRITISCH"
    };

    println!("=== eGPU Manager Status ===");
    println!(
        "Warnstufe: {}  |  Modus: {}  |  Uptime: {}s  |  Recovery: {}",
        warning,
        mode,
        uptime,
        if recovery { "AKTIV" } else { "nein" }
    );
    println!(
        "Health Score: {:.0}/100 ({})  |  Events: {}",
        hs_score, hs_indicator, hs_events
    );
    println!();

    if let Some(gpus) = status["gpus"].as_array() {
        for gpu in gpus {
            let name = gpu["name"].as_str().unwrap_or("?");
            let pci = gpu["pci_address"].as_str().unwrap_or("?");
            let gpu_type = gpu["type"].as_str().unwrap_or("?");
            let temp = gpu["temperature_c"].as_u64().unwrap_or(0);
            let util = gpu["utilization_gpu_percent"].as_u64().unwrap_or(0);
            let mem_used = gpu["memory_used_mb"].as_u64().unwrap_or(0);
            let mem_total = gpu["memory_total_mb"].as_u64().unwrap_or(0);
            let power = gpu["power_draw_w"].as_f64().unwrap_or(0.0);

            println!(
                "  [{:7}] {} ({})",
                gpu_type, name, pci
            );
            println!(
                "           {}°C | GPU {}% | VRAM {}/{} MB | {:.0}W",
                temp, util, mem_used, mem_total, power
            );
        }
    }

    if let Some(remotes) = status["remote_gpus"].as_array() {
        for r in remotes {
            let name = r["name"].as_str().unwrap_or("?");
            let status = r["status"].as_str().unwrap_or("?");
            let latency = r["latency_ms"].as_u64().unwrap_or(0);
            println!("  [remote ] {} — {} ({}ms)", name, status, latency);
        }
    }
    println!();
}

fn print_pipelines(data: &serde_json::Value) {
    if let Some(pipelines) = data.as_array() {
        println!("=== Pipelines ===");
        for p in pipelines {
            let project = p["project"].as_str().unwrap_or("?");
            let container = p["container"].as_str().unwrap_or("?");
            let prio = p["priority"].as_u64().unwrap_or(0);
            let gpu = p["gpu_type"].as_str().unwrap_or("–");
            let vram = p["actual_vram_mb"].as_u64().unwrap_or(0);
            let workloads = p["workload_types"]
                .as_array()
                .map(|a| {
                    a.iter()
                        .filter_map(|v| v.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                })
                .unwrap_or_else(|| "–".to_string());
            let reason = p["decision_reason"].as_str().unwrap_or("");

            println!(
                "  [P{}] {}/{} — GPU: {} | VRAM: {} MB | {} | {}",
                prio, project, container, gpu, vram, workloads, reason
            );
        }
    }
}

async fn cmd_priority_set(container: &str, priority: u32) -> anyhow::Result<()> {
    if priority == 0 || priority > 5 {
        anyhow::bail!("Priorität muss zwischen 1 und 5 liegen");
    }

    let client = reqwest::Client::new();
    let resp = client
        .put(format!("{DAEMON_URL}/api/pipelines/{container}/priority"))
        .json(&serde_json::json!({"priority": priority}))
        .send()
        .await?;

    if resp.status().is_success() {
        println!("Priorität von {container} auf {priority} gesetzt.");
    } else {
        let body: serde_json::Value = resp.json().await.unwrap_or_default();
        eprintln!(
            "Fehler: {}",
            body["error"].as_str().unwrap_or("Unbekannter Fehler")
        );
    }

    Ok(())
}

async fn cmd_priority_get(container: &str) -> anyhow::Result<()> {
    let client = reqwest::Client::new();
    let resp = client
        .get(format!("{DAEMON_URL}/api/pipelines/{container}"))
        .send()
        .await?;

    if resp.status().is_success() {
        let body: serde_json::Value = resp.json().await?;
        let prio = body["priority"].as_u64().unwrap_or(0);
        println!("{container}: Priorität {prio}");
    } else {
        eprintln!("Pipeline {container} nicht gefunden");
    }

    Ok(())
}

async fn cmd_config_reload() -> anyhow::Result<()> {
    let client = reqwest::Client::new();
    let resp = client
        .post(format!("{DAEMON_URL}/api/config/reload"))
        .send()
        .await?;

    if resp.status().is_success() {
        println!("Konfiguration neu geladen.");
    } else {
        eprintln!("Reload fehlgeschlagen.");
    }

    Ok(())
}

async fn cmd_wizard_list() -> anyhow::Result<()> {
    let client = reqwest::Client::new();
    let resp = client
        .get(format!("{DAEMON_URL}/api/pipelines"))
        .send()
        .await;

    match resp {
        Ok(r) if r.status().is_success() => {
            let body: serde_json::Value = r.json().await?;
            if let Some(pipelines) = body.as_array() {
                println!("Konfigurierte Pipelines:");
                for p in pipelines {
                    let project = p["project"].as_str().unwrap_or("?");
                    let container = p["container"].as_str().unwrap_or("?");
                    let prio = p["priority"].as_u64().unwrap_or(0);
                    println!("  {} / {} (Prio {})", project, container, prio);
                }
            }
        }
        _ => {
            eprintln!("Daemon nicht erreichbar. Zeige Pipelines aus Konfiguration...");
            // Fallback: Config direkt lesen
            let config_paths = [
                "/etc/egpu-manager/config.toml",
                "config.toml",
            ];
            for path in &config_paths {
                if let Ok(config) = egpu_manager_common::config::Config::load(std::path::Path::new(path)) {
                    println!("Pipelines (aus {path}):");
                    for p in &config.pipeline {
                        println!("  {} / {} (Prio {})", p.project, p.container, p.gpu_priority);
                    }
                    return Ok(());
                }
            }
            eprintln!("Keine Konfiguration gefunden.");
        }
    }

    Ok(())
}
