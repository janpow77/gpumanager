//! Windows 11 Remote-Node Setup-ZIP Generator.
//! Generates an offline-capable ZIP package for installing
//! Ollama + egpu-agent on a Windows 11 remote GPU node.

use std::io::Write;
use std::net::IpAddr;

use anyhow::Result;
use sha2::{Digest, Sha256};
use zip::write::SimpleFileOptions;
use zip::ZipWriter;

/// NUC IP detection: returns the first non-loopback IPv4 address.
fn detect_nuc_ip() -> String {
    // Try to get from environment or fallback to hostname resolution
    if let Ok(output) = std::process::Command::new("hostname")
        .arg("-I")
        .output()
    {
        let stdout = String::from_utf8_lossy(&output.stdout);
        for part in stdout.split_whitespace() {
            if let Ok(ip) = part.parse::<IpAddr>() {
                if !ip.is_loopback() {
                    if let IpAddr::V4(_) = ip {
                        return ip.to_string();
                    }
                }
            }
        }
    }
    "192.168.1.100".to_string()
}

/// Generate a random auth token (hex-encoded 32 bytes).
fn generate_token() -> String {
    use rand::Rng;
    let mut rng = rand::rng();
    let bytes: [u8; 32] = rng.random();
    hex::encode(bytes)
}

/// Generate the setup ZIP in memory and return the bytes.
pub fn generate_setup_zip() -> Result<Vec<u8>> {
    let nuc_ip = detect_nuc_ip();
    let token = generate_token();

    let buf = Vec::new();
    let mut zip = ZipWriter::new(std::io::Cursor::new(buf));
    let opts = SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated)
        .unix_permissions(0o644);

    let prefix = "egpu-remote-setup";

    // Collect all files for checksum generation
    let mut files: Vec<(String, Vec<u8>)> = Vec::new();

    // README.txt
    let readme = format!(
        r#"========================================
  eGPU Remote-Node Setup
  Generiert auf NUC: {nuc_ip}
========================================

INSTALLATION (Windows 11):

1. Dieses ZIP nach C:\egpu-remote\setup\ entpacken
2. PowerShell als Administrator oeffnen
3. Ausfuehren:

   cd C:\egpu-remote\setup\egpu-remote-setup
   Set-ExecutionPolicy -Scope Process -ExecutionPolicy Bypass
   .\install.ps1

VORAUSSETZUNGEN:
- Windows 11
- Administratorrechte
- NVIDIA-Treiber >= 576.02
- 20 GB freier Speicherplatz
- NUC erreichbar unter {nuc_ip}

DEINSTALLATION:
   .\uninstall.ps1

Bei Fragen: GPU-Manager Weboberflaeche http://{nuc_ip}:7842
"#
    );
    files.push((format!("{prefix}/README.txt"), readme.into_bytes()));

    // config/egpu-agent-config.toml
    let agent_config = format!(
        r#"# eGPU Agent Konfiguration
# Generiert am: {ts}

[nuc]
host = "{nuc_ip}"
api_port = 7843
local_api_port = 7842

[agent]
port = 8899
heartbeat_interval_seconds = 30

[ollama]
host = "0.0.0.0"
port = 11434
"#,
        ts = chrono::Utc::now().format("%Y-%m-%d %H:%M:%S UTC")
    );
    files.push((
        format!("{prefix}/config/egpu-agent-config.toml"),
        agent_config.into_bytes(),
    ));

    // config/auth-token.secret
    files.push((
        format!("{prefix}/config/auth-token.secret"),
        token.as_bytes().to_vec(),
    ));

    // config/ollama-config.json
    let ollama_config = format!(
        r#"{{
  "host": "0.0.0.0:11434",
  "origins": ["{nuc_ip}"],
  "models_path": "C:\\egpu-remote\\ollama\\models",
  "keep_alive": "24h"
}}
"#
    );
    files.push((
        format!("{prefix}/config/ollama-config.json"),
        ollama_config.into_bytes(),
    ));

    // config/firewall-rules.ps1
    let firewall = format!(
        r#"# Firewall-Regeln fuer eGPU Remote-Node
# Nur Zugriff von NUC-IP {nuc_ip}

# Ollama API (Port 11434)
New-NetFirewallRule -DisplayName "eGPU-Remote: Ollama API" `
  -Direction Inbound -Action Allow -Protocol TCP -LocalPort 11434 `
  -RemoteAddress {nuc_ip} -Profile Any

# eGPU Agent (Port 8899)
New-NetFirewallRule -DisplayName "eGPU-Remote: Agent" `
  -Direction Inbound -Action Allow -Protocol TCP -LocalPort 8899 `
  -RemoteAddress {nuc_ip} -Profile Any

# Agent Health (Port 7843)
New-NetFirewallRule -DisplayName "eGPU-Remote: Agent Health" `
  -Direction Inbound -Action Allow -Protocol TCP -LocalPort 7843 `
  -RemoteAddress {nuc_ip} -Profile Any

Write-Host "Firewall-Regeln fuer eGPU-Remote erstellt (nur {nuc_ip})" -ForegroundColor Green
"#
    );
    files.push((
        format!("{prefix}/config/firewall-rules.ps1"),
        firewall.into_bytes(),
    ));

    // install.ps1
    let install_ps1 = format!(
        r#"#Requires -RunAsAdministrator
# ============================================================================
# eGPU Remote-Node Installer fuer Windows 11
# NUC: {nuc_ip} | Generiert: {ts}
# ============================================================================

$ErrorActionPreference = "Stop"
$ProgressPreference = "SilentlyContinue"
$InstallDir = "C:\egpu-remote"
$LogFile = "$InstallDir\install.log"
$ProgressFile = "$InstallDir\install-progress.json"

function Log($msg) {{
    $ts = Get-Date -Format "yyyy-MM-dd HH:mm:ss"
    "$ts $msg" | Tee-Object -FilePath $LogFile -Append
}}

function Save-Progress($step) {{
    @{{ step = $step; timestamp = (Get-Date).ToString("o") }} | ConvertTo-Json | Set-Content $ProgressFile
}}

Write-Host ""
Write-Host "========================================" -ForegroundColor Cyan
Write-Host "  eGPU Remote-Node Installer" -ForegroundColor Cyan
Write-Host "  NUC: {nuc_ip}" -ForegroundColor Cyan
Write-Host "========================================" -ForegroundColor Cyan
Write-Host ""

# --- Schritt 0: Verzeichnisse erstellen ---
Log "Schritt 0: Verzeichnisse vorbereiten"
New-Item -ItemType Directory -Path $InstallDir -Force | Out-Null
New-Item -ItemType Directory -Path "$InstallDir\ollama" -Force | Out-Null
New-Item -ItemType Directory -Path "$InstallDir\ollama\models" -Force | Out-Null
New-Item -ItemType Directory -Path "$InstallDir\agent" -Force | Out-Null
New-Item -ItemType Directory -Path "$InstallDir\config" -Force | Out-Null

# --- Schritt 1: Voraussetzungen pruefen ---
Log "Schritt 1: Voraussetzungen pruefen"

# Windows 11 Check
$os = (Get-CimInstance Win32_OperatingSystem).Caption
if ($os -notlike "*Windows 11*" -and $os -notlike "*Windows Server*") {{
    Write-Host "WARNUNG: $os erkannt. Empfohlen: Windows 11" -ForegroundColor Yellow
}}

# NVIDIA-Treiber Check
$nvidiaSmi = "C:\Windows\System32\nvidia-smi.exe"
if (Test-Path $nvidiaSmi) {{
    $nvOut = & $nvidiaSmi --query-gpu=driver_version,name,memory.total --format=csv,noheader 2>$null
    if ($nvOut) {{
        Write-Host "  GPU erkannt: $nvOut" -ForegroundColor Green
        Log "  GPU: $nvOut"
    }}
}} else {{
    Write-Host "  WARNUNG: nvidia-smi nicht gefunden. NVIDIA-Treiber installieren!" -ForegroundColor Red
    Write-Host "  Download: https://www.nvidia.com/Download/index.aspx" -ForegroundColor Yellow
    Read-Host "  Enter druecken nach Treiberinstallation (oder Ctrl+C zum Abbrechen)"
}}

# Speicherplatz
$disk = Get-CimInstance Win32_LogicalDisk -Filter "DeviceID='C:'"
$freeGB = [math]::Round($disk.FreeSpace / 1GB, 1)
if ($freeGB -lt 20) {{
    Write-Host "  WARNUNG: Nur $freeGB GB frei. Mindestens 20 GB empfohlen." -ForegroundColor Yellow
}}
Write-Host "  Speicherplatz: $freeGB GB frei" -ForegroundColor Green

# NUC erreichbar?
$nucReachable = Test-Connection -ComputerName {nuc_ip} -Count 1 -Quiet -ErrorAction SilentlyContinue
if ($nucReachable) {{
    Write-Host "  NUC ({nuc_ip}): erreichbar" -ForegroundColor Green
}} else {{
    Write-Host "  NUC ({nuc_ip}): NICHT erreichbar" -ForegroundColor Yellow
    Write-Host "  Installation wird fortgesetzt, Registrierung spaeter moeglich." -ForegroundColor Yellow
}}

Save-Progress 1

# --- Schritt 2: Konfiguration kopieren ---
Log "Schritt 2: Konfiguration kopieren"
$scriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
Copy-Item "$scriptDir\config\*" "$InstallDir\config\" -Force -Recurse
Write-Host "  Konfiguration nach $InstallDir\config kopiert" -ForegroundColor Green
Save-Progress 2

# --- Schritt 3: Ollama installieren ---
Log "Schritt 3: Ollama installieren"
$ollamaExe = "$InstallDir\ollama\ollama.exe"

if (Test-Path $ollamaExe) {{
    Write-Host "  Ollama bereits vorhanden: $ollamaExe" -ForegroundColor Green
}} else {{
    # Check if installer was provided in ZIP
    $localInstaller = "$scriptDir\installers\ollama-windows-amd64.exe"
    if (Test-Path $localInstaller) {{
        Write-Host "  Ollama-Installer aus Paket verwenden..." -ForegroundColor Cyan
        Copy-Item $localInstaller $ollamaExe
    }} else {{
        # Download
        $ollamaUrl = "https://github.com/ollama/ollama/releases/latest/download/ollama-windows-amd64.exe"
        Write-Host "  Ollama wird heruntergeladen..." -ForegroundColor Cyan
        Write-Host "  URL: $ollamaUrl" -ForegroundColor Gray
        try {{
            Invoke-WebRequest -Uri $ollamaUrl -OutFile $ollamaExe -UseBasicParsing
            Write-Host "  Ollama heruntergeladen" -ForegroundColor Green
        }} catch {{
            Write-Host "  FEHLER beim Download: $_" -ForegroundColor Red
            Write-Host "  Bitte manuell herunterladen und nach $ollamaExe kopieren" -ForegroundColor Yellow
        }}
    }}
}}

# Ollama als Service einrichten (NSSM oder sc.exe)
$ollamaSvcName = "OllamaEgpuRemote"
$svc = Get-Service -Name $ollamaSvcName -ErrorAction SilentlyContinue
if ($null -eq $svc) {{
    Write-Host "  Ollama-Service wird erstellt..." -ForegroundColor Cyan
    # Environment fuer Ollama
    [System.Environment]::SetEnvironmentVariable("OLLAMA_HOST", "0.0.0.0:11434", "Machine")
    [System.Environment]::SetEnvironmentVariable("OLLAMA_MODELS", "$InstallDir\ollama\models", "Machine")

    # Service mit sc.exe erstellen
    & sc.exe create $ollamaSvcName binPath= "$ollamaExe serve" start= auto DisplayName= "Ollama (eGPU Remote)"
    & sc.exe description $ollamaSvcName "Ollama LLM Server fuer eGPU Remote-Node"
    Start-Service $ollamaSvcName
    Write-Host "  Ollama-Service gestartet" -ForegroundColor Green
}} else {{
    Write-Host "  Ollama-Service existiert bereits" -ForegroundColor Green
    if ($svc.Status -ne 'Running') {{
        Start-Service $ollamaSvcName
        Write-Host "  Ollama-Service gestartet" -ForegroundColor Green
    }}
}}
Save-Progress 3

# --- Schritt 4: Firewall-Regeln ---
Log "Schritt 4: Firewall-Regeln"
& "$InstallDir\config\firewall-rules.ps1"
Save-Progress 4

# --- Schritt 5: Registrierung am NUC ---
Log "Schritt 5: Registrierung am NUC"
$token = Get-Content "$InstallDir\config\auth-token.secret" -Raw
$token = $token.Trim()

# GPU-Informationen sammeln
$gpuName = "Unknown GPU"
$vramMb = 0
if (Test-Path $nvidiaSmi) {{
    $gpuInfo = & $nvidiaSmi --query-gpu=name,memory.total --format=csv,noheader,nounits 2>$null
    if ($gpuInfo) {{
        $parts = $gpuInfo -split ","
        $gpuName = $parts[0].Trim()
        $vramMb = [int]$parts[1].Trim()
    }}
}}

$hostname = $env:COMPUTERNAME
$regBody = @{{
    name = "remote-$hostname"
    host = (Get-NetIPAddress -AddressFamily IPv4 | Where-Object {{ $_.IPAddress -notlike "127.*" }} | Select-Object -First 1).IPAddress
    port_ollama = 11434
    port_agent = 8899
    gpu_name = $gpuName
    vram_mb = $vramMb
    token = $token
}} | ConvertTo-Json

if ($nucReachable) {{
    try {{
        $headers = @{{ "Authorization" = "Bearer $token"; "Content-Type" = "application/json" }}
        $resp = Invoke-RestMethod -Uri "http://{nuc_ip}:7843/api/remote/register" -Method Post -Body $regBody -Headers $headers
        Write-Host "  Registrierung erfolgreich! Remote-GPU erscheint im NUC-Dashboard." -ForegroundColor Green
        Log "  Registrierung: OK"
    }} catch {{
        Write-Host "  Registrierung fehlgeschlagen: $_" -ForegroundColor Yellow
        Write-Host "  Kann spaeter manuell wiederholt werden." -ForegroundColor Yellow
        Log "  Registrierung: fehlgeschlagen - $_"
    }}
}} else {{
    Write-Host "  NUC nicht erreichbar - Registrierung uebersprungen" -ForegroundColor Yellow
    Write-Host "  Spaeter manuell: Invoke-RestMethod -Uri http://{nuc_ip}:7843/api/remote/register -Method Post -Body '$regBody' -Headers @{{Authorization='Bearer $token'}}" -ForegroundColor Gray
}}
Save-Progress 5

# --- Zusammenfassung ---
Write-Host ""
Write-Host "========================================" -ForegroundColor Green
Write-Host "  Installation abgeschlossen!" -ForegroundColor Green
Write-Host "========================================" -ForegroundColor Green
Write-Host ""
Write-Host "  Ollama: http://localhost:11434" -ForegroundColor Cyan
Write-Host "  NUC:    http://{nuc_ip}:7842" -ForegroundColor Cyan
Write-Host "  GPU:    $gpuName ($vramMb MB)" -ForegroundColor Cyan
Write-Host "  Log:    $LogFile" -ForegroundColor Gray
Write-Host ""

Log "Installation abgeschlossen"
Save-Progress "done"
"#,
        ts = chrono::Utc::now().format("%Y-%m-%d %H:%M:%S UTC")
    );
    files.push((format!("{prefix}/install.ps1"), install_ps1.into_bytes()));

    // uninstall.ps1
    let uninstall_ps1 = format!(
        r#"#Requires -RunAsAdministrator
# eGPU Remote-Node Deinstallation

$ErrorActionPreference = "Continue"
$InstallDir = "C:\egpu-remote"

Write-Host "eGPU Remote-Node wird deinstalliert..." -ForegroundColor Yellow

# Service stoppen und entfernen
$svcName = "OllamaEgpuRemote"
$svc = Get-Service -Name $svcName -ErrorAction SilentlyContinue
if ($null -ne $svc) {{
    Stop-Service $svcName -Force -ErrorAction SilentlyContinue
    & sc.exe delete $svcName
    Write-Host "  Ollama-Service entfernt" -ForegroundColor Green
}}

# Firewall-Regeln entfernen
Remove-NetFirewallRule -DisplayName "eGPU-Remote: Ollama API" -ErrorAction SilentlyContinue
Remove-NetFirewallRule -DisplayName "eGPU-Remote: Agent" -ErrorAction SilentlyContinue
Remove-NetFirewallRule -DisplayName "eGPU-Remote: Agent Health" -ErrorAction SilentlyContinue
Write-Host "  Firewall-Regeln entfernt" -ForegroundColor Green

# NUC abmelden
$tokenFile = "$InstallDir\config\auth-token.secret"
if (Test-Path $tokenFile) {{
    $token = (Get-Content $tokenFile -Raw).Trim()
    try {{
        $headers = @{{ "Authorization" = "Bearer $token"; "Content-Type" = "application/json" }}
        Invoke-RestMethod -Uri "http://{nuc_ip}:7843/api/remote/unregister" -Method Post -Headers $headers -ErrorAction SilentlyContinue
        Write-Host "  NUC-Abmeldung gesendet" -ForegroundColor Green
    }} catch {{
        Write-Host "  NUC-Abmeldung fehlgeschlagen (nicht kritisch)" -ForegroundColor Yellow
    }}
}}

# Umgebungsvariablen entfernen
[System.Environment]::SetEnvironmentVariable("OLLAMA_HOST", $null, "Machine")
[System.Environment]::SetEnvironmentVariable("OLLAMA_MODELS", $null, "Machine")

Write-Host ""
Write-Host "Deinstallation abgeschlossen." -ForegroundColor Green
Write-Host "Verzeichnis $InstallDir kann manuell geloescht werden." -ForegroundColor Gray
"#
    );
    files.push((
        format!("{prefix}/uninstall.ps1"),
        uninstall_ps1.into_bytes(),
    ));

    // Generate SHA256SUMS.txt
    let mut checksums = String::new();
    for (name, data) in &files {
        let mut hasher = Sha256::new();
        hasher.update(data);
        let hash = hex::encode(hasher.finalize());
        let short_name = name.strip_prefix(&format!("{prefix}/")).unwrap_or(name);
        checksums.push_str(&format!("{hash}  {short_name}\n"));
    }
    files.push((
        format!("{prefix}/checksums/SHA256SUMS.txt"),
        checksums.into_bytes(),
    ));

    // Write all files to ZIP
    for (name, data) in &files {
        zip.start_file(name, opts)?;
        zip.write_all(data)?;
    }

    let cursor = zip.finish()?;
    Ok(cursor.into_inner())
}
