use clap::Parser;
use std::path::Path;

const DAEMON_URL: &str = "http://127.0.0.1:7842";
const DEFAULT_CONFIG_PATH: &str = "/etc/egpu-manager/config.toml";
const DEFAULT_BACKUP_DIR: &str = "/etc/egpu-manager/backups";
const DEFAULT_TOKEN_PATH: &str = "/etc/egpu-manager/remote-token.secret";

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
    /// Konfigurationsdatei validieren (Schema, Pipelines, LLM Gateway)
    Validate {
        /// Pfad zur config.toml (default: /etc/egpu-manager/config.toml)
        #[arg(short, long)]
        path: Option<String>,
    },
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
            ConfigAction::Validate { path } => cmd_config_validate(path)?,
            ConfigAction::Rollback { timestamp } => cmd_config_rollback(timestamp).await?,
            ConfigAction::ListBackups => cmd_config_list_backups()?,
        },
        Commands::Remote { action } => match action {
            RemoteAction::Init => cmd_remote_init()?,
            RemoteAction::RotateToken => cmd_remote_rotate_token()?,
            RemoteAction::ShowToken => cmd_remote_show_token()?,
        },
        Commands::Wizard { action } => match action {
            WizardAction::Add { path } => cmd_wizard_add(&path)?,
            WizardAction::Remove { project } => cmd_wizard_remove(&project)?,
            WizardAction::Edit { container } => cmd_wizard_edit(&container).await?,
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

// ============================================================
// Config: Validate
// ============================================================

fn cmd_config_validate(path: Option<String>) -> anyhow::Result<()> {
    use egpu_manager_common::config::Config;

    let config_path = path
        .as_deref()
        .unwrap_or(DEFAULT_CONFIG_PATH);

    println!("Validiere: {config_path}");

    // 1. Datei lesbar?
    let content = match std::fs::read_to_string(config_path) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("  FEHLER: Datei nicht lesbar: {e}");
            std::process::exit(1);
        }
    };

    // 2. TOML-Syntax gültig?
    let config: Config = match toml::from_str(&content) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("  FEHLER: TOML-Parsing fehlgeschlagen:");
            eprintln!("  {e}");
            std::process::exit(1);
        }
    };

    println!("  Schema v{} — OK", config.schema_version);

    // 3. Pipelines prüfen
    let mut warnings = Vec::new();
    let mut info_msgs = Vec::new();

    if config.pipeline.is_empty() {
        warnings.push("Keine Pipelines konfiguriert".to_string());
    } else {
        info_msgs.push(format!("{} Pipeline(s) konfiguriert", config.pipeline.len()));

        for p in &config.pipeline {
            if p.gpu_priority < 1 || p.gpu_priority > 5 {
                warnings.push(format!(
                    "Pipeline '{}': gpu_priority {} außerhalb 1-5",
                    p.container, p.gpu_priority
                ));
            }
            if p.vram_estimate_mb == 0 {
                warnings.push(format!(
                    "Pipeline '{}': vram_estimate_mb ist 0 (ungewöhnlich)",
                    p.container
                ));
            }
        }
    }

    // 4. GPU-Config prüfen
    if config.gpu.poll_interval_seconds == 0 {
        warnings.push("gpu.poll_interval_secs ist 0 (Polling deaktiviert)".to_string());
    }

    // 5. LLM Gateway prüfen
    if let Some(ref gw) = config.llm_gateway {
        info_msgs.push(format!(
            "LLM Gateway: {} Provider, {} App-Routings",
            gw.providers.len(),
            gw.app_routing.len()
        ));

        for app in &gw.app_routing {
            if !app.preferred_provider.is_empty() {
                let provider_exists = gw.providers.iter().any(|p| p.name == app.preferred_provider);
                if !provider_exists {
                    warnings.push(format!(
                        "App '{}': preferred_provider '{}' existiert nicht in providers",
                        app.app_id, app.preferred_provider
                    ));
                }
            }
        }
    }

    // 6. Ollama-Instanzen
    let instance_count = config.ollama_instance.len();
    if instance_count > 0 {
        info_msgs.push(format!("{instance_count} Ollama-Instanz(en) konfiguriert"));
    }

    // 7. Remote-GPU prüfen
    let remote_count = config.remote_gpu.len();
    if remote_count > 0 {
        info_msgs.push(format!("{remote_count} Remote-GPU(s) konfiguriert"));
    }

    // Ausgabe
    for msg in &info_msgs {
        println!("  {msg}");
    }

    if warnings.is_empty() {
        println!("\n  Validierung erfolgreich — keine Probleme gefunden.");
    } else {
        println!("\n  {} Warnung(en):", warnings.len());
        for w in &warnings {
            println!("    ! {w}");
        }
    }

    Ok(())
}

// ============================================================
// Config: Rollback & List-Backups
// ============================================================

/// Alle Backup-Dateien finden (aus mehreren Verzeichnissen)
fn find_backup_files() -> Vec<std::path::PathBuf> {
    let mut backups = Vec::new();

    // Backups-Verzeichnis: /etc/egpu-manager/backups/
    if let Ok(entries) = std::fs::read_dir(DEFAULT_BACKUP_DIR) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().is_some_and(|e| e == "toml") || path.to_string_lossy().contains("config.toml.bak") {
                backups.push(path);
            }
        }
    }

    // Direkte .bak.*-Dateien neben config.toml
    let config_dir = Path::new(DEFAULT_CONFIG_PATH).parent().unwrap_or(Path::new("/etc/egpu-manager"));
    if let Ok(entries) = std::fs::read_dir(config_dir) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with("config.toml.bak") {
                backups.push(entry.path());
            }
        }
    }

    // Fallback: ~/.config/egpu-manager/
    if let Some(home) = std::env::var_os("HOME") {
        let user_dir = Path::new(&home).join(".config/egpu-manager");
        if let Ok(entries) = std::fs::read_dir(&user_dir) {
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().to_string();
                if name.contains("config") && (name.contains(".bak") || name.contains("backup")) {
                    backups.push(entry.path());
                }
            }
        }
    }

    // Lokales Verzeichnis
    if let Ok(entries) = std::fs::read_dir(".") {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with("config.toml.bak") {
                backups.push(entry.path());
            }
        }
    }

    backups.sort();
    backups.dedup();
    backups
}

fn cmd_config_list_backups() -> anyhow::Result<()> {
    let backups = find_backup_files();

    if backups.is_empty() {
        println!("Keine Backups gefunden.");
        println!("Suchpfade:");
        println!("  - {DEFAULT_BACKUP_DIR}/");
        println!("  - /etc/egpu-manager/config.toml.bak.*");
        if let Some(home) = std::env::var_os("HOME") {
            println!("  - {}/.config/egpu-manager/", Path::new(&home).display());
        }
        println!("  - ./config.toml.bak.*");
        return Ok(());
    }

    println!("=== Konfiguration-Backups ===");
    println!("{:<60} {:>10}  {}", "Datei", "Groesse", "Geaendert");
    println!("{}", "-".repeat(90));

    for path in &backups {
        let meta = std::fs::metadata(path);
        let size = meta.as_ref().map(|m| m.len()).unwrap_or(0);
        let modified = meta
            .as_ref()
            .ok()
            .and_then(|m| m.modified().ok())
            .map(|t| {
                let dt: chrono::DateTime<chrono::Local> = t.into();
                dt.format("%Y-%m-%d %H:%M:%S").to_string()
            })
            .unwrap_or_else(|| "?".to_string());

        let size_str = if size > 1024 {
            format!("{:.1} KB", size as f64 / 1024.0)
        } else {
            format!("{} B", size)
        };

        println!("{:<60} {:>10}  {}", path.display(), size_str, modified);
    }

    println!("\nZum Wiederherstellen: egpu-manager config rollback [DATEINAME_ODER_TIMESTAMP]");
    Ok(())
}

async fn cmd_config_rollback(timestamp: Option<String>) -> anyhow::Result<()> {
    let backups = find_backup_files();

    if backups.is_empty() {
        anyhow::bail!("Keine Backups gefunden. Siehe: egpu-manager config list-backups");
    }

    // Backup auswaehlen
    let backup_path = if let Some(ts) = &timestamp {
        // Suche nach Backup mit passendem Timestamp oder Dateinamen
        backups
            .iter()
            .find(|p| {
                let name = p.to_string_lossy();
                name.contains(ts.as_str())
            })
            .cloned()
            .ok_or_else(|| anyhow::anyhow!(
                "Kein Backup mit '{}' gefunden. Verfuegbare Backups:\n{}",
                ts,
                backups.iter().map(|p| format!("  {}", p.display())).collect::<Vec<_>>().join("\n")
            ))?
    } else {
        // Neuestes Backup (letzte Datei nach Sortierung / Aenderungszeit)
        backups
            .iter()
            .max_by_key(|p| {
                std::fs::metadata(p)
                    .and_then(|m| m.modified())
                    .unwrap_or(std::time::SystemTime::UNIX_EPOCH)
            })
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("Kein Backup gefunden"))?
    };

    println!("Stelle Backup wieder her: {}", backup_path.display());

    // Backup-Inhalt lesen und validieren
    let backup_content = std::fs::read_to_string(&backup_path)
        .map_err(|e| anyhow::anyhow!("Backup nicht lesbar: {e}"))?;

    // TOML-Validierung: Pruefen ob es gueltige Konfiguration ist
    let _: toml::Value = toml::from_str(&backup_content)
        .map_err(|e| anyhow::anyhow!("Backup enthaelt kein gueltiges TOML: {e}"))?;

    // Aktuelles Config sichern bevor wir ueberschreiben
    let config_path = Path::new(DEFAULT_CONFIG_PATH);
    if config_path.exists() {
        let now = chrono::Local::now().format("%Y%m%d_%H%M%S");
        let pre_rollback = format!("{}.pre-rollback.{}", DEFAULT_CONFIG_PATH, now);
        if let Err(e) = std::fs::copy(config_path, &pre_rollback) {
            eprintln!("Warnung: Konnte aktuelle Config nicht sichern: {e}");
        } else {
            println!("Aktuelle Konfiguration gesichert als: {pre_rollback}");
        }
    }

    // Backup kopieren
    match std::fs::copy(&backup_path, config_path) {
        Ok(_) => {
            println!("Konfiguration wiederhergestellt aus: {}", backup_path.display());
        }
        Err(e) => {
            eprintln!("Fehler beim Kopieren (evtl. sudo noetig): {e}");
            eprintln!("Manuell ausfuehren:");
            eprintln!("  sudo cp {} {}", backup_path.display(), DEFAULT_CONFIG_PATH);
            return Ok(());
        }
    }

    // Daemon neu laden
    println!("Lade Daemon-Konfiguration neu...");
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .build()?;
    match client
        .post(format!("{DAEMON_URL}/api/config/reload"))
        .send()
        .await
    {
        Ok(r) if r.status().is_success() => {
            println!("Daemon hat Konfiguration neu geladen.");
        }
        Ok(r) => {
            eprintln!("Daemon-Reload fehlgeschlagen: HTTP {}", r.status());
        }
        Err(_) => {
            eprintln!("Daemon nicht erreichbar. Manuell neu laden:");
            eprintln!("  sudo systemctl restart egpu-manager");
        }
    }

    Ok(())
}

// ============================================================
// Remote: Token-Verwaltung
// ============================================================

/// Zufaelligen 32-Byte Hex-Token generieren
fn generate_token() -> String {
    use rand::Rng;
    let mut rng = rand::rng();
    let bytes: [u8; 32] = rng.random();
    bytes.iter().map(|b| format!("{:02x}", b)).collect()
}

fn write_token_file(token: &str) -> Result<(), String> {
    let token_path = Path::new(DEFAULT_TOKEN_PATH);

    // Verzeichnis erstellen falls noetig
    if let Some(parent) = token_path.parent() {
        if !parent.exists() {
            if let Err(e) = std::fs::create_dir_all(parent) {
                return Err(format!("Verzeichnis {} nicht erstellbar: {e}", parent.display()));
            }
        }
    }

    // Token schreiben mit restriktiven Berechtigungen
    match std::fs::write(token_path, format!("{token}\n")) {
        Ok(()) => {
            // Berechtigungen setzen: nur root lesbar
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = std::fs::set_permissions(token_path, std::fs::Permissions::from_mode(0o600));
            }
            Ok(())
        }
        Err(e) => Err(format!("Konnte {DEFAULT_TOKEN_PATH} nicht schreiben: {e}")),
    }
}

fn cmd_remote_init() -> anyhow::Result<()> {
    let token = generate_token();

    println!("=== Remote-Setup initialisieren ===");
    println!();

    match write_token_file(&token) {
        Ok(()) => {
            println!("Token geschrieben nach: {DEFAULT_TOKEN_PATH}");
        }
        Err(e) => {
            eprintln!("Warnung: {e}");
            eprintln!("Evtl. mit sudo ausfuehren, oder Token manuell speichern.");
            println!();
            println!("Token (manuell speichern):");
            println!("  {token}");
            println!();
            println!("  sudo mkdir -p /etc/egpu-manager");
            println!("  echo '{token}' | sudo tee {DEFAULT_TOKEN_PATH}");
            println!("  sudo chmod 600 {DEFAULT_TOKEN_PATH}");
        }
    }

    println!();
    println!("--- Anleitung fuer Remote-Knoten ---");
    println!();
    println!("1. config.toml anpassen:");
    println!("   [remote]");
    println!("   enabled = true");
    println!("   bind = \"0.0.0.0\"");
    println!("   port = 7843");
    println!("   token_path = \"{DEFAULT_TOKEN_PATH}\"");
    println!();
    println!("2. Auf dem Remote-Knoten den gleichen Token konfigurieren.");
    println!();
    println!("3. Daemon neu starten:");
    println!("   sudo systemctl restart egpu-manager");
    println!();
    println!("Generierter Token: {token}");

    Ok(())
}

fn cmd_remote_rotate_token() -> anyhow::Result<()> {
    let token = generate_token();

    println!("=== Token rotieren ===");

    // Pruefen ob alter Token existiert
    if Path::new(DEFAULT_TOKEN_PATH).exists() {
        println!("Alter Token wird ersetzt.");
    }

    match write_token_file(&token) {
        Ok(()) => {
            println!("Neuer Token geschrieben nach: {DEFAULT_TOKEN_PATH}");
            println!();
            println!("Neuer Token: {token}");
            println!();
            println!("WICHTIG: Token muss auch auf allen Remote-Knoten aktualisiert werden!");
            println!("Danach Daemon neu starten: sudo systemctl restart egpu-manager");
        }
        Err(e) => {
            eprintln!("Fehler: {e}");
            eprintln!();
            println!("Neuer Token (manuell speichern): {token}");
            println!("  echo '{token}' | sudo tee {DEFAULT_TOKEN_PATH}");
        }
    }

    Ok(())
}

fn cmd_remote_show_token() -> anyhow::Result<()> {
    let token_path = Path::new(DEFAULT_TOKEN_PATH);

    if !token_path.exists() {
        eprintln!("Token-Datei nicht gefunden: {DEFAULT_TOKEN_PATH}");
        eprintln!("Remote-Setup initialisieren mit: egpu-manager remote init");
        std::process::exit(1);
    }

    match std::fs::read_to_string(token_path) {
        Ok(content) => {
            let token = content.trim();
            if token.is_empty() {
                eprintln!("Token-Datei ist leer: {DEFAULT_TOKEN_PATH}");
                eprintln!("Neuen Token generieren mit: egpu-manager remote rotate-token");
                std::process::exit(1);
            }
            println!("{token}");
        }
        Err(e) => {
            eprintln!("Token-Datei nicht lesbar: {e}");
            eprintln!("Evtl. mit sudo ausfuehren: sudo egpu-manager remote show-token");
            std::process::exit(1);
        }
    }

    Ok(())
}

// ============================================================
// Wizard: Add, Remove, Edit
// ============================================================

/// Erkennt GPU-Nutzung in einer docker-compose.yml (String-basiert, kein YAML-Parser noetig)
struct ComposeAnalysis {
    services: Vec<String>,
    uses_nvidia_runtime: bool,
    has_cuda_env: bool,
    has_gpu_deploy: bool,
    env_hints: Vec<String>,
}

fn analyze_compose_file(content: &str) -> ComposeAnalysis {
    let mut analysis = ComposeAnalysis {
        services: Vec::new(),
        uses_nvidia_runtime: false,
        has_cuda_env: false,
        has_gpu_deploy: false,
        env_hints: Vec::new(),
    };

    // Services erkennen (Zeilen unter "services:" die nicht eingerueckt sind relativ)
    let mut in_services = false;
    let mut current_indent = 0usize;
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed == "services:" || trimmed.starts_with("services:") {
            in_services = true;
            current_indent = line.len() - line.trim_start().len();
            continue;
        }
        if in_services && !trimmed.is_empty() && !trimmed.starts_with('#') {
            let indent = line.len() - line.trim_start().len();
            // Service-Name: genau eine Ebene unter services
            if indent == current_indent + 2 && trimmed.ends_with(':') {
                let name = trimmed.trim_end_matches(':').to_string();
                analysis.services.push(name);
            }
            // Neue Top-Level-Sektion beendet services
            if indent <= current_indent && !trimmed.is_empty() {
                in_services = false;
            }
        }
    }

    // GPU-Marker erkennen
    let content_lower = content.to_lowercase();
    analysis.uses_nvidia_runtime = content_lower.contains("runtime: nvidia")
        || content_lower.contains("runtime: \"nvidia\"")
        || content_lower.contains("runtime: 'nvidia'");
    analysis.has_cuda_env = content_lower.contains("nvidia_visible_devices")
        || content_lower.contains("cuda_visible_devices")
        || content_lower.contains("nvidia_driver_capabilities");
    analysis.has_gpu_deploy = content_lower.contains("capabilities: [gpu]")
        || content_lower.contains("- gpu")
            && content_lower.contains("capabilities:");

    // Workload-Hinweise
    if content_lower.contains("ollama") {
        analysis.env_hints.push("inference".to_string());
    }
    if content_lower.contains("celery") || content_lower.contains("worker") {
        analysis.env_hints.push("batch".to_string());
    }
    if content_lower.contains("ocr") || content_lower.contains("tesseract") || content_lower.contains("paddleocr") {
        analysis.env_hints.push("ocr".to_string());
    }
    if content_lower.contains("embedding") {
        analysis.env_hints.push("embeddings".to_string());
    }
    if content_lower.contains("torch") || content_lower.contains("pytorch") || content_lower.contains("tensorflow") {
        analysis.env_hints.push("training".to_string());
    }
    if content_lower.contains("transcri") || content_lower.contains("whisper") {
        analysis.env_hints.push("transcription".to_string());
    }

    analysis
}

fn cmd_wizard_add(path: &str) -> anyhow::Result<()> {
    let path = Path::new(path);

    // docker-compose.yml finden
    let compose_path = if path.is_file() {
        path.to_path_buf()
    } else if path.is_dir() {
        // In Verzeichnis nach compose-Dateien suchen
        let candidates = [
            "docker-compose.yml",
            "docker-compose.yaml",
            "compose.yml",
            "compose.yaml",
        ];
        candidates
            .iter()
            .map(|name| path.join(name))
            .find(|p| p.exists())
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "Keine docker-compose.yml gefunden in: {}\nGesucht: {}",
                    path.display(),
                    candidates.join(", ")
                )
            })?
    } else {
        anyhow::bail!("Pfad existiert nicht: {}", path.display());
    };

    println!("=== Projekt-Wizard: Neues Projekt ===");
    println!("Compose-Datei: {}", compose_path.display());
    println!();

    let content = std::fs::read_to_string(&compose_path)
        .map_err(|e| anyhow::anyhow!("Compose-Datei nicht lesbar: {e}"))?;

    let analysis = analyze_compose_file(&content);

    // Projektname aus Verzeichnisname ableiten
    let project_name = compose_path
        .parent()
        .and_then(|p| p.file_name())
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "mein_projekt".to_string());

    let uses_gpu = analysis.uses_nvidia_runtime || analysis.has_cuda_env || analysis.has_gpu_deploy;

    println!("Erkannte Services: {}", analysis.services.join(", "));
    println!(
        "GPU-Nutzung erkannt: {}",
        if uses_gpu { "Ja" } else { "Nein" }
    );
    if analysis.uses_nvidia_runtime {
        println!("  - NVIDIA Runtime konfiguriert");
    }
    if analysis.has_cuda_env {
        println!("  - CUDA Umgebungsvariablen gefunden");
    }
    if analysis.has_gpu_deploy {
        println!("  - GPU Deploy-Ressourcen konfiguriert");
    }
    if !analysis.env_hints.is_empty() {
        println!("  - Erkannte Workload-Typen: {}", analysis.env_hints.join(", "));
    }
    println!();

    // Pipeline-Bloecke generieren
    let workload_types = if analysis.env_hints.is_empty() && uses_gpu {
        vec!["compute".to_string()]
    } else if analysis.env_hints.is_empty() {
        vec!["cpu".to_string()]
    } else {
        analysis.env_hints.clone()
    };

    let gpu_services: Vec<&str> = if uses_gpu {
        // Alle Services sind potenziell GPU-faehig
        analysis.services.iter().map(|s| s.as_str()).collect()
    } else {
        Vec::new()
    };

    // Mindestens einen Block generieren (fuer den ersten GPU-Service oder alle)
    let services_to_add = if gpu_services.is_empty() {
        // Kein GPU erkannt, trotzdem fuer den ersten Service generieren
        if analysis.services.is_empty() {
            vec!["main".to_string()]
        } else {
            vec![analysis.services[0].clone()]
        }
    } else {
        gpu_services.iter().map(|s| s.to_string()).collect()
    };

    println!("--- Generierte Pipeline-Konfiguration ---");
    println!("Folgendes in {} einfuegen:", DEFAULT_CONFIG_PATH);
    println!();

    let compose_abs = std::fs::canonicalize(&compose_path)
        .unwrap_or_else(|_| compose_path.clone());

    for service in &services_to_add {
        let container_name = format!("{}_{}", project_name, service).replace('-', "_");
        let wt_str = workload_types
            .iter()
            .map(|w| format!("\"{}\"", w))
            .collect::<Vec<_>>()
            .join(", ");

        println!("[[pipeline]]");
        println!("project = \"{}\"", project_name);
        println!("container = \"{}\"", container_name);
        println!("compose_file = \"{}\"", compose_abs.display());
        println!("compose_service = \"{}\"", service);
        println!("workload_types = [{}]", wt_str);
        println!("gpu_priority = 3");
        println!("gpu_device = \"0000:05:00.0\"       # eGPU PCI-Adresse anpassen!");
        println!("cuda_fallback_device = \"0000:02:00.0\"  # Interne GPU anpassen!");
        if uses_gpu {
            println!("vram_estimate_mb = 4096            # VRAM-Bedarf anpassen!");
        } else {
            println!("vram_estimate_mb = 0");
        }
        println!();
    }

    println!("--- Hinweise ---");
    println!("- PCI-Adressen anpassen (siehe: lspci | grep -i vga)");
    println!("- gpu_priority: 1=Kritisch, 2=Hoch, 3=Normal, 4=Niedrig, 5=Minimal");
    println!("- vram_estimate_mb: Geschaetzter VRAM-Verbrauch in MB");
    println!("- Nach dem Einfuegen: egpu-manager config reload");

    Ok(())
}

fn cmd_wizard_remove(project: &str) -> anyhow::Result<()> {
    println!("=== Projekt-Wizard: Projekt entfernen ===");
    println!();

    // Config lesen und passende Pipelines finden
    let config_paths = [DEFAULT_CONFIG_PATH, "config.toml"];

    let mut found_config = None;
    let mut found_path = "";

    for path in &config_paths {
        if let Ok(content) = std::fs::read_to_string(path) {
            found_config = Some(content);
            found_path = path;
            break;
        }
    }

    let Some(config_content) = found_config else {
        eprintln!("Keine Konfiguration gefunden.");
        eprintln!("Gesucht in: {}", config_paths.join(", "));
        return Ok(());
    };

    // Config parsen
    let config: toml::Value = toml::from_str(&config_content)
        .map_err(|e| anyhow::anyhow!("Konfiguration nicht parsbar: {e}"))?;

    let pipelines = config
        .get("pipeline")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    let matching: Vec<&toml::Value> = pipelines
        .iter()
        .filter(|p| {
            p.get("project")
                .and_then(|v| v.as_str())
                .is_some_and(|s| s == project)
                || p.get("container")
                    .and_then(|v| v.as_str())
                    .is_some_and(|s| s == project)
        })
        .collect();

    if matching.is_empty() {
        println!("Kein Projekt/Container mit dem Namen '{}' gefunden.", project);
        println!();
        println!("Vorhandene Pipelines:");
        for p in &pipelines {
            let proj = p.get("project").and_then(|v| v.as_str()).unwrap_or("?");
            let cont = p.get("container").and_then(|v| v.as_str()).unwrap_or("?");
            println!("  {} / {}", proj, cont);
        }
        return Ok(());
    }

    println!("Gefundene Eintraege in {} :", found_path);
    println!();
    for p in &matching {
        let proj = p.get("project").and_then(|v| v.as_str()).unwrap_or("?");
        let cont = p.get("container").and_then(|v| v.as_str()).unwrap_or("?");
        let prio = p
            .get("gpu_priority")
            .and_then(|v| v.as_integer())
            .unwrap_or(0);
        let compose = p
            .get("compose_file")
            .and_then(|v| v.as_str())
            .unwrap_or("?");

        println!("  [[pipeline]]");
        println!("  project = \"{}\"", proj);
        println!("  container = \"{}\"", cont);
        println!("  gpu_priority = {}", prio);
        println!("  compose_file = \"{}\"", compose);
        println!();
    }

    println!("--- Anleitung zum Entfernen ---");
    println!();
    println!(
        "Die oben genannten [[pipeline]]-Bloecke aus {} entfernen.",
        found_path
    );
    println!("Dann Konfiguration neu laden:");
    println!("  egpu-manager config reload");
    println!();
    println!(
        "Tipp: Vorher ein Backup erstellen:");
    println!(
        "  sudo cp {} {}.bak.$(date +%Y%m%d_%H%M%S)",
        found_path, found_path
    );

    Ok(())
}

async fn cmd_wizard_edit(container: &str) -> anyhow::Result<()> {
    println!("=== Projekt-Wizard: Container bearbeiten ===");
    println!();

    // Aktuelle Pipeline-Info vom Daemon holen
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .build()?;

    let resp = client
        .get(format!("{DAEMON_URL}/api/pipelines/{container}"))
        .send()
        .await;

    match resp {
        Ok(r) if r.status().is_success() => {
            let body: serde_json::Value = r.json().await?;

            let project = body["project"].as_str().unwrap_or("?");
            let prio = body["priority"].as_u64().unwrap_or(0);
            let gpu_type = body["gpu_type"].as_str().unwrap_or("?");
            let vram = body["actual_vram_mb"].as_u64().unwrap_or(0);
            let workloads = body["workload_types"]
                .as_array()
                .map(|a| {
                    a.iter()
                        .filter_map(|v| v.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                })
                .unwrap_or_else(|| "-".to_string());
            let compose = body["compose_file"].as_str().unwrap_or("?");
            let service = body["compose_service"].as_str().unwrap_or("?");
            let gpu_device = body["gpu_device"].as_str().unwrap_or("?");
            let fallback = body["cuda_fallback_device"].as_str().unwrap_or("?");
            let reason = body["decision_reason"].as_str().unwrap_or("");

            println!("Container: {container}");
            println!("Projekt:   {project}");
            println!("Prioritaet: {prio}");
            println!("GPU-Typ:   {gpu_type}");
            println!("VRAM:      {vram} MB");
            println!("Workloads: {workloads}");
            println!("Compose:   {compose}");
            println!("Service:   {service}");
            println!("GPU:       {gpu_device}");
            println!("Fallback:  {fallback}");
            if !reason.is_empty() {
                println!("Routing:   {reason}");
            }
        }
        Ok(r) => {
            eprintln!("Pipeline '{}' nicht gefunden (HTTP {})", container, r.status());

            // Fallback: Config direkt lesen
            if let Ok(config) = egpu_manager_common::config::Config::load(Path::new(DEFAULT_CONFIG_PATH)) {
                if let Some(p) = config.pipeline.iter().find(|p| p.container == container) {
                    println!("Aus Konfiguration ({DEFAULT_CONFIG_PATH}):");
                    println!("  Projekt:    {}", p.project);
                    println!("  Container:  {}", p.container);
                    println!("  Prioritaet: {}", p.gpu_priority);
                    println!("  Compose:    {}", p.compose_file);
                    println!("  Service:    {}", p.compose_service);
                    println!("  GPU:        {}", p.gpu_device);
                    println!("  Fallback:   {}", p.cuda_fallback_device);
                    println!("  VRAM:       {} MB", p.vram_estimate_mb);
                    println!("  Workloads:  {}", p.workload_types.join(", "));
                } else {
                    eprintln!("Container '{}' auch nicht in Konfiguration gefunden.", container);
                    return Ok(());
                }
            } else {
                return Ok(());
            }
        }
        Err(_) => {
            eprintln!("Daemon nicht erreichbar auf {DAEMON_URL}");
            // Fallback: direkt Config lesen
            if let Ok(config) = egpu_manager_common::config::Config::load(Path::new(DEFAULT_CONFIG_PATH)) {
                if let Some(p) = config.pipeline.iter().find(|p| p.container == container) {
                    println!("Aus Konfiguration:");
                    println!("  Projekt:    {}", p.project);
                    println!("  Container:  {}", p.container);
                    println!("  Prioritaet: {}", p.gpu_priority);
                } else {
                    eprintln!("Container '{}' nicht gefunden.", container);
                    return Ok(());
                }
            }
        }
    }

    println!();
    println!("--- Bearbeitung ---");
    println!();
    println!("Pipeline-Einstellungen in {DEFAULT_CONFIG_PATH} bearbeiten.");
    println!("Den entsprechenden [[pipeline]]-Block suchen und anpassen.");
    println!();
    println!("Haeufige Aenderungen:");
    println!("  gpu_priority = <1-5>         # 1=Kritisch, 5=Minimal");
    println!("  vram_estimate_mb = <MB>      # VRAM-Schaetzung");
    println!("  gpu_device = \"DDDD:BB:DD.F\"  # PCI-Adresse der GPU");
    println!("  exclusive_gpu = true/false   # Exklusiver GPU-Zugriff");
    println!();
    println!("Schnell-Aenderung der Prioritaet (ohne Config-Edit):");
    println!("  egpu-manager priority set {container} <1-5>");
    println!();
    println!("Nach Config-Aenderung:");
    println!("  egpu-manager config reload");

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
