//! Windows 11 Remote-Node Setup package generator.
//! Builds a ZIP package for installing Ollama and a heartbeat-based
//! remote node registration task on a Windows machine.

use std::fs;
use std::io::{Cursor, Write};
use std::net::IpAddr;
use std::path::Path;

use anyhow::{Context, Result};
use egpu_manager_common::config::Config;
use sha2::{Digest, Sha256};
use zip::write::SimpleFileOptions;
use zip::ZipWriter;

const DEFAULT_REMOTE_OLLAMA_PORT: u16 = 11434;
const DEFAULT_REMOTE_AGENT_PORT: u16 = 8899;
const ARCHIVE_PREFIX: &str = "egpu-remote-setup";

#[derive(Debug, Clone, Default)]
pub struct SetupGenerateRequest {
    pub remote_name: Option<String>,
    pub nuc_host: Option<String>,
}

#[derive(Debug, Clone)]
pub struct GeneratedSetupPackage {
    pub filename: String,
    pub zip_bytes: Vec<u8>,
}

#[derive(Debug, Clone)]
struct SetupSpec {
    remote_name: String,
    nuc_host: String,
    local_api_port: u16,
    remote_listener_port: u16,
    remote_ollama_port: u16,
    remote_agent_port: u16,
    remote_token: String,
    ollama_version_pin: Option<String>,
}

/// NUC IP detection: returns the first non-loopback IPv4 address.
fn detect_nuc_host() -> String {
    if let Ok(output) = std::process::Command::new("hostname").arg("-I").output() {
        let stdout = String::from_utf8_lossy(&output.stdout);
        for part in stdout.split_whitespace() {
            if let Ok(ip) = part.parse::<IpAddr>()
                && !ip.is_loopback()
                && matches!(ip, IpAddr::V4(_))
            {
                return ip.to_string();
            }
        }
    }
    "192.168.1.100".to_string()
}

fn sanitize_remote_name(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut last_dash = false;

    for ch in raw.trim().chars() {
        if ch.is_ascii_alphanumeric() || ch == '_' {
            out.push(ch);
            last_dash = false;
        } else if !last_dash {
            out.push('-');
            last_dash = true;
        }
    }

    let trimmed = out.trim_matches('-');
    if trimmed.is_empty() {
        "remote-node".to_string()
    } else {
        trimmed.to_string()
    }
}

fn generate_token() -> String {
    use rand::Rng;
    let mut rng = rand::rng();
    let bytes: [u8; 32] = rng.random();
    hex::encode(bytes)
}

fn ensure_remote_token(token_path: &str) -> Result<String> {
    if token_path.trim().is_empty() {
        anyhow::bail!(
            "remote.token_path ist leer; fuer den Windows-Installer wird ein persistenter Remote-Token benoetigt"
        );
    }

    let path = Path::new(token_path);
    if path.exists() {
        let token = fs::read_to_string(path)
            .with_context(|| format!("Remote-Token-Datei nicht lesbar: {}", path.display()))?;
        let token = token.trim().to_string();
        if token.is_empty() {
            anyhow::bail!("Remote-Token-Datei ist leer: {}", path.display());
        }
        return Ok(token);
    }

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_context(|| {
            format!(
                "Verzeichnis fuer Remote-Token konnte nicht erstellt werden: {}",
                parent.display()
            )
        })?;
    }

    let token = generate_token();
    fs::write(path, format!("{token}\n"))
        .with_context(|| format!("Remote-Token konnte nicht gespeichert werden: {}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o600));
    }
    Ok(token)
}

fn build_spec(config: &Config, request: SetupGenerateRequest) -> Result<SetupSpec> {
    let remote_cfg = config
        .remote
        .as_ref()
        .filter(|cfg| cfg.enabled)
        .context("Remote-Listener ist nicht aktiviert; [remote] fehlt oder enabled=false")?;

    let remote_name = sanitize_remote_name(
        request
            .remote_name
            .as_deref()
            .unwrap_or("remote-node"),
    );
    let nuc_host = request
        .nuc_host
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .unwrap_or_else(detect_nuc_host);

    let matching_remote = config.remote_gpu.iter().find(|gpu| {
        gpu.name == remote_name || sanitize_remote_name(&gpu.name) == remote_name
    });

    Ok(SetupSpec {
        remote_name,
        nuc_host,
        local_api_port: config.local_api.port,
        remote_listener_port: remote_cfg.port,
        remote_ollama_port: matching_remote
            .map(|gpu| gpu.port_ollama)
            .unwrap_or(DEFAULT_REMOTE_OLLAMA_PORT),
        remote_agent_port: matching_remote
            .map(|gpu| gpu.port_egpu_agent)
            .unwrap_or(DEFAULT_REMOTE_AGENT_PORT),
        remote_token: ensure_remote_token(&remote_cfg.token_path)?,
        ollama_version_pin: (!remote_cfg.ollama_version_pin.trim().is_empty())
            .then(|| remote_cfg.ollama_version_pin.trim().to_string()),
    })
}

fn package_filename(remote_name: &str) -> String {
    format!("egpu-remote-setup-{remote_name}.zip")
}

fn ollama_download_url(version_pin: Option<&str>) -> String {
    if let Some(version) = version_pin {
        format!(
            "https://github.com/ollama/ollama/releases/download/v{version}/ollama-windows-amd64.exe"
        )
    } else {
        "https://github.com/ollama/ollama/releases/latest/download/ollama-windows-amd64.exe"
            .to_string()
    }
}

pub fn generate_setup_package(
    config: &Config,
    request: SetupGenerateRequest,
) -> Result<GeneratedSetupPackage> {
    let spec = build_spec(config, request)?;
    let zip_bytes = generate_setup_zip(&spec)?;
    Ok(GeneratedSetupPackage {
        filename: package_filename(&spec.remote_name),
        zip_bytes,
    })
}

fn generate_setup_zip(spec: &SetupSpec) -> Result<Vec<u8>> {
    let buf = Vec::new();
    let mut zip = ZipWriter::new(Cursor::new(buf));
    let opts = SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated)
        .unix_permissions(0o644);

    let generated_at = chrono::Utc::now().format("%Y-%m-%d %H:%M:%S UTC");
    let ollama_url = ollama_download_url(spec.ollama_version_pin.as_deref());

    let mut files: Vec<(String, Vec<u8>)> = Vec::new();

    let readme = format!(
        r#"========================================
  eGPU Remote-Node Setup
  Node: {remote_name}
  Primary: {nuc_host}
========================================

INSTALLATION (Windows 11):

1. Dieses ZIP nach C:\egpu-remote\setup\ entpacken
2. PowerShell als Administrator oeffnen
3. Ausfuehren:

   cd C:\egpu-remote\setup\{prefix}
   Set-ExecutionPolicy -Scope Process -ExecutionPolicy Bypass
   .\install.ps1

WAS INSTALLIERT WIRD:
- Ollama fuer Remote-LLM-Workloads
- Heartbeat-/Registrierungs-Task zum NUC
- Firewall-Regeln fuer {nuc_host}

DEINSTALLATION:
   .\uninstall.ps1

DASHBOARD:
   http://{nuc_host}:{local_api_port}
"#,
        remote_name = spec.remote_name,
        nuc_host = spec.nuc_host,
        prefix = ARCHIVE_PREFIX,
        local_api_port = spec.local_api_port,
    );
    files.push((format!("{ARCHIVE_PREFIX}/README.txt"), readme.into_bytes()));

    let agent_config = format!(
        r#"# eGPU Remote-Node Konfiguration
# Generiert am: {generated_at}

[node]
name = "{remote_name}"

[nuc]
host = "{nuc_host}"
remote_listener_port = {remote_listener_port}
local_api_port = {local_api_port}

[agent]
heartbeat_interval_seconds = 30
port = {remote_agent_port}

[ollama]
host = "0.0.0.0"
port = {remote_ollama_port}
"#,
        generated_at = generated_at,
        remote_name = spec.remote_name,
        nuc_host = spec.nuc_host,
        remote_listener_port = spec.remote_listener_port,
        local_api_port = spec.local_api_port,
        remote_agent_port = spec.remote_agent_port,
        remote_ollama_port = spec.remote_ollama_port,
    );
    files.push((
        format!("{ARCHIVE_PREFIX}/config/egpu-agent-config.toml"),
        agent_config.into_bytes(),
    ));

    files.push((
        format!("{ARCHIVE_PREFIX}/config/auth-token.secret"),
        spec.remote_token.as_bytes().to_vec(),
    ));

    let ollama_config = format!(
        r#"{{
  "host": "0.0.0.0:{remote_ollama_port}",
  "origins": ["{nuc_host}"],
  "models_path": "C:\\egpu-remote\\ollama\\models",
  "keep_alive": "24h"
}}
"#,
        remote_ollama_port = spec.remote_ollama_port,
        nuc_host = spec.nuc_host,
    );
    files.push((
        format!("{ARCHIVE_PREFIX}/config/ollama-config.json"),
        ollama_config.into_bytes(),
    ));

    let firewall_rules = format!(
        r#"# Firewall-Regeln fuer eGPU Remote-Node
# Nur Zugriff von NUC/IP {nuc_host}

New-NetFirewallRule -DisplayName "eGPU-Remote: Ollama API" `
  -Direction Inbound -Action Allow -Protocol TCP -LocalPort {remote_ollama_port} `
  -RemoteAddress {nuc_host} -Profile Any

New-NetFirewallRule -DisplayName "eGPU-Remote: Heartbeat" `
  -Direction Outbound -Action Allow -Protocol TCP -RemotePort {remote_listener_port} `
  -RemoteAddress {nuc_host} -Profile Any

Write-Host "Firewall-Regeln fuer eGPU-Remote erstellt (nur {nuc_host})" -ForegroundColor Green
"#,
        nuc_host = spec.nuc_host,
        remote_ollama_port = spec.remote_ollama_port,
        remote_listener_port = spec.remote_listener_port,
    );
    files.push((
        format!("{ARCHIVE_PREFIX}/config/firewall-rules.ps1"),
        firewall_rules.into_bytes(),
    ));

    let heartbeat_script = format!(
        r#"$ErrorActionPreference = "Continue"
$InstallDir = "C:\egpu-remote"
$LogFile = "$InstallDir\agent\heartbeat.log"
$TokenFile = "$InstallDir\config\auth-token.secret"
$NucHost = "{nuc_host}"
$RemoteListenerPort = {remote_listener_port}
$RemoteName = "{remote_name}"
$RemoteOllamaPort = {remote_ollama_port}
$RemoteAgentPort = {remote_agent_port}
$HeartbeatIntervalSeconds = 30

param([switch]$RunOnce)

function Log($msg) {{
  $ts = Get-Date -Format "yyyy-MM-dd HH:mm:ss"
  "$ts $msg" | Out-File -FilePath $LogFile -Append -Encoding utf8
}}

function Get-PrimaryIPv4() {{
  $address = Get-NetIPAddress -AddressFamily IPv4 -ErrorAction SilentlyContinue |
    Where-Object {{ $_.IPAddress -notlike "127.*" -and $_.IPAddress -notlike "169.254.*" }} |
    Sort-Object InterfaceMetric |
    Select-Object -First 1 -ExpandProperty IPAddress
  if (-not $address) {{ $address = "127.0.0.1" }}
  return $address
}}

function Get-GpuInfo() {{
  $name = "Unknown GPU"
  $vramMb = 0
  $nvidiaSmi = "C:\Windows\System32\nvidia-smi.exe"
  if (Test-Path $nvidiaSmi) {{
    $gpuInfo = & $nvidiaSmi --query-gpu=name,memory.total --format=csv,noheader,nounits 2>$null | Select-Object -First 1
    if ($gpuInfo) {{
      $parts = $gpuInfo -split ","
      $name = $parts[0].Trim()
      if ($parts.Length -gt 1) {{ $vramMb = [int]$parts[1].Trim() }}
    }}
  }}
  return @{{ name = $name; vram_mb = $vramMb }}
}}

function Get-LatencyMs() {{
  try {{
    $ping = Test-Connection -ComputerName $NucHost -Count 1 -ErrorAction Stop | Select-Object -First 1
    return [int][math]::Round($ping.Latency)
  }} catch {{
    return $null
  }}
}}

function Invoke-RemoteApi($path, $body) {{
  $token = (Get-Content $TokenFile -Raw).Trim()
  $headers = @{{ "Authorization" = "Bearer $token"; "Content-Type" = "application/json" }}
  $uri = "http://$NucHost:$RemoteListenerPort$path"
  Invoke-RestMethod -Uri $uri -Method Post -Headers $headers -Body ($body | ConvertTo-Json -Compress)
}}

function Send-Heartbeat() {{
  $gpu = Get-GpuInfo
  $hostIp = Get-PrimaryIPv4
  $latency = Get-LatencyMs

  $heartbeatBody = @{{
    name = $RemoteName
    latency_ms = $latency
    gpu_name = $gpu.name
    vram_mb = $gpu.vram_mb
  }}

  try {{
    Invoke-RemoteApi "/api/remote/heartbeat" $heartbeatBody | Out-Null
    Log "Heartbeat OK ($hostIp, latency=${{latency}}ms)"
    return
  }} catch {{
    Log "Heartbeat fehlgeschlagen, versuche Register: $_"
  }}

  $registerBody = @{{
    name = $RemoteName
    host = $hostIp
    port_ollama = $RemoteOllamaPort
    port_agent = $RemoteAgentPort
    gpu_name = $gpu.name
    vram_mb = $gpu.vram_mb
  }}

  try {{
    Invoke-RemoteApi "/api/remote/register" $registerBody | Out-Null
    Log "Register OK ($hostIp)"
  }} catch {{
    Log "Register fehlgeschlagen: $_"
  }}
}}

do {{
  Send-Heartbeat
  if (-not $RunOnce) {{
    Start-Sleep -Seconds $HeartbeatIntervalSeconds
  }}
}} while (-not $RunOnce)
"#,
        nuc_host = spec.nuc_host,
        remote_listener_port = spec.remote_listener_port,
        remote_name = spec.remote_name,
        remote_ollama_port = spec.remote_ollama_port,
        remote_agent_port = spec.remote_agent_port,
    );
    files.push((
        format!("{ARCHIVE_PREFIX}/agent/heartbeat-loop.ps1"),
        heartbeat_script.into_bytes(),
    ));

    let service_ollama_xml = format!(
        r#"<service>
  <id>OllamaEgpuRemote</id>
  <name>Ollama (eGPU Remote)</name>
  <description>Ollama remote node service for {remote_name}</description>
  <executable>C:\egpu-remote\ollama\ollama.exe</executable>
  <arguments>serve</arguments>
</service>
"#,
        remote_name = spec.remote_name
    );
    files.push((
        format!("{ARCHIVE_PREFIX}/services/ollama-service.xml"),
        service_ollama_xml.into_bytes(),
    ));

    let heartbeat_task_xml = format!(
        r#"<Task>
  <RegistrationInfo>
    <Description>eGPU Remote heartbeat for {remote_name}</Description>
  </RegistrationInfo>
  <Triggers>
    <BootTrigger />
  </Triggers>
  <Actions Context="Author">
    <Exec>
      <Command>PowerShell.exe</Command>
      <Arguments>-NoProfile -ExecutionPolicy Bypass -File "C:\egpu-remote\agent\heartbeat-loop.ps1"</Arguments>
    </Exec>
  </Actions>
</Task>
"#,
        remote_name = spec.remote_name
    );
    files.push((
        format!("{ARCHIVE_PREFIX}/services/egpu-heartbeat-task.xml"),
        heartbeat_task_xml.into_bytes(),
    ));

    let installers_readme = format!(
        r#"OPTIONALE OFFLINE-INSTALLER
===========================

Fuer einen komplett offline-faehigen USB-Installationsablauf koennen hier
zusaetzlich Dateien abgelegt werden:

- ollama-windows-amd64.exe

Wenn diese Datei vorhanden ist, nutzt install.ps1 sie statt eines Downloads.
Aktuell vorgesehene Quelle:
{ollama_url}
"#,
        ollama_url = ollama_url
    );
    files.push((
        format!("{ARCHIVE_PREFIX}/installers/README.txt"),
        installers_readme.into_bytes(),
    ));

    let install_ps1 = format!(
        r#"#Requires -RunAsAdministrator
$ErrorActionPreference = "Stop"
$ProgressPreference = "SilentlyContinue"
$InstallDir = "C:\egpu-remote"
$LogFile = "$InstallDir\install.log"
$ProgressFile = "$InstallDir\install-progress.json"
$TaskName = "eGPURemoteHeartbeat"

function Log($msg) {{
    $ts = Get-Date -Format "yyyy-MM-dd HH:mm:ss"
    "$ts $msg" | Tee-Object -FilePath $LogFile -Append
}}

function Save-Progress($step) {{
    @{{ step = $step; timestamp = (Get-Date).ToString("o") }} | ConvertTo-Json | Set-Content $ProgressFile
}}

function Verify-Checksums($rootDir) {{
    $checksumFile = Join-Path $rootDir "checksums\SHA256SUMS.txt"
    if (-not (Test-Path $checksumFile)) {{
        throw "SHA256SUMS.txt fehlt"
    }}

    Get-Content $checksumFile | ForEach-Object {{
        if (-not $_) {{ return }}
        $parts = $_ -split "\s+", 2
        $expected = $parts[0].Trim()
        $relativePath = $parts[1].Trim()
        $filePath = Join-Path $rootDir $relativePath
        if (-not (Test-Path $filePath)) {{
            throw "Datei fehlt: $relativePath"
        }}
        $actual = (Get-FileHash -Algorithm SHA256 $filePath).Hash.ToLowerInvariant()
        if ($actual -ne $expected.ToLowerInvariant()) {{
            throw "Hash-Mismatch: $relativePath"
        }}
    }}
}}

Write-Host ""
Write-Host "========================================" -ForegroundColor Cyan
Write-Host "  eGPU Remote-Node Installer" -ForegroundColor Cyan
Write-Host "  Node: {remote_name}" -ForegroundColor Cyan
Write-Host "  Primary: {nuc_host}" -ForegroundColor Cyan
Write-Host "========================================" -ForegroundColor Cyan
Write-Host ""

$scriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
New-Item -ItemType Directory -Path $InstallDir -Force | Out-Null
New-Item -ItemType Directory -Path "$InstallDir\ollama" -Force | Out-Null
New-Item -ItemType Directory -Path "$InstallDir\ollama\models" -Force | Out-Null
New-Item -ItemType Directory -Path "$InstallDir\agent" -Force | Out-Null
New-Item -ItemType Directory -Path "$InstallDir\config" -Force | Out-Null

Log "Schritt 0: Integritaet pruefen"
Verify-Checksums $scriptDir
Save-Progress 0

Log "Schritt 1: Voraussetzungen pruefen"
$os = (Get-CimInstance Win32_OperatingSystem).Caption
if ($os -notlike "*Windows 11*" -and $os -notlike "*Windows Server*") {{
    Write-Host "WARNUNG: $os erkannt. Empfohlen: Windows 11" -ForegroundColor Yellow
}}

$nvidiaSmi = "C:\Windows\System32\nvidia-smi.exe"
if (Test-Path $nvidiaSmi) {{
    $nvOut = & $nvidiaSmi --query-gpu=driver_version,name,memory.total --format=csv,noheader 2>$null
    if ($nvOut) {{
        Write-Host "  GPU erkannt: $nvOut" -ForegroundColor Green
        Log "GPU: $nvOut"
    }}
}} else {{
    Write-Host "WARNUNG: nvidia-smi nicht gefunden. NVIDIA-Treiber installieren." -ForegroundColor Yellow
}}

$disk = Get-CimInstance Win32_LogicalDisk -Filter "DeviceID='C:'"
$freeGB = [math]::Round($disk.FreeSpace / 1GB, 1)
Write-Host "  Speicherplatz: $freeGB GB frei" -ForegroundColor Green
if ($freeGB -lt 20) {{
    Write-Host "WARNUNG: Mindestens 20 GB freier Speicherplatz empfohlen." -ForegroundColor Yellow
}}
Save-Progress 1

Log "Schritt 2: Konfiguration kopieren"
Copy-Item "$scriptDir\config\*" "$InstallDir\config\" -Force -Recurse
Copy-Item "$scriptDir\agent\*" "$InstallDir\agent\" -Force -Recurse
Save-Progress 2

Log "Schritt 3: Ollama installieren"
$ollamaExe = "$InstallDir\ollama\ollama.exe"
$localInstaller = "$scriptDir\installers\ollama-windows-amd64.exe"
if (Test-Path $localInstaller) {{
    Copy-Item $localInstaller $ollamaExe -Force
    Write-Host "  Lokalen Ollama-Installer aus dem Paket verwendet." -ForegroundColor Green
}} elseif (-not (Test-Path $ollamaExe)) {{
    Write-Host "  Ollama wird heruntergeladen..." -ForegroundColor Cyan
    Invoke-WebRequest -Uri "{ollama_url}" -OutFile $ollamaExe -UseBasicParsing
}}

[System.Environment]::SetEnvironmentVariable("OLLAMA_HOST", "0.0.0.0:{remote_ollama_port}", "Machine")
[System.Environment]::SetEnvironmentVariable("OLLAMA_MODELS", "$InstallDir\ollama\models", "Machine")

$ollamaSvcName = "OllamaEgpuRemote"
$svc = Get-Service -Name $ollamaSvcName -ErrorAction SilentlyContinue
if ($null -eq $svc) {{
    & sc.exe create $ollamaSvcName binPath= "$ollamaExe serve" start= auto DisplayName= "Ollama (eGPU Remote)"
    & sc.exe description $ollamaSvcName "Ollama LLM Server fuer eGPU Remote-Node"
}}
Start-Service $ollamaSvcName -ErrorAction SilentlyContinue
Save-Progress 3

Log "Schritt 4: Firewall-Regeln"
& "$InstallDir\config\firewall-rules.ps1"
Save-Progress 4

Log "Schritt 5: Heartbeat-Task einrichten"
Unregister-ScheduledTask -TaskName $TaskName -Confirm:$false -ErrorAction SilentlyContinue | Out-Null
$action = New-ScheduledTaskAction -Execute "PowerShell.exe" -Argument "-NoProfile -ExecutionPolicy Bypass -File `"$InstallDir\agent\heartbeat-loop.ps1`""
$trigger = New-ScheduledTaskTrigger -AtStartup
$settings = New-ScheduledTaskSettingsSet -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries -RestartCount 3 -RestartInterval (New-TimeSpan -Minutes 1)
Register-ScheduledTask -TaskName $TaskName -Action $action -Trigger $trigger -Settings $settings -User "SYSTEM" -RunLevel Highest -Force | Out-Null
& "$InstallDir\agent\heartbeat-loop.ps1" -RunOnce
Start-ScheduledTask -TaskName $TaskName
Save-Progress 5

Write-Host ""
Write-Host "========================================" -ForegroundColor Green
Write-Host "  Installation abgeschlossen!" -ForegroundColor Green
Write-Host "========================================" -ForegroundColor Green
Write-Host "  Node:   {remote_name}" -ForegroundColor Cyan
Write-Host "  Ollama: http://localhost:{remote_ollama_port}" -ForegroundColor Cyan
Write-Host "  NUC:    http://{nuc_host}:{local_api_port}" -ForegroundColor Cyan
Write-Host "  Log:    $LogFile" -ForegroundColor Gray

Log "Installation abgeschlossen"
Save-Progress "done"
"#,
        remote_name = spec.remote_name,
        nuc_host = spec.nuc_host,
        ollama_url = ollama_url,
        remote_ollama_port = spec.remote_ollama_port,
        local_api_port = spec.local_api_port,
    );
    files.push((format!("{ARCHIVE_PREFIX}/install.ps1"), install_ps1.into_bytes()));

    let uninstall_ps1 = format!(
        r#"#Requires -RunAsAdministrator
$ErrorActionPreference = "Continue"
$InstallDir = "C:\egpu-remote"
$TaskName = "eGPURemoteHeartbeat"
$NucHost = "{nuc_host}"
$RemoteListenerPort = {remote_listener_port}
$RemoteName = "{remote_name}"

Write-Host "eGPU Remote-Node wird deinstalliert..." -ForegroundColor Yellow

Stop-Service "OllamaEgpuRemote" -Force -ErrorAction SilentlyContinue
& sc.exe delete "OllamaEgpuRemote" | Out-Null

Unregister-ScheduledTask -TaskName $TaskName -Confirm:$false -ErrorAction SilentlyContinue | Out-Null

Remove-NetFirewallRule -DisplayName "eGPU-Remote: Ollama API" -ErrorAction SilentlyContinue
Remove-NetFirewallRule -DisplayName "eGPU-Remote: Heartbeat" -ErrorAction SilentlyContinue

$tokenFile = "$InstallDir\config\auth-token.secret"
if (Test-Path $tokenFile) {{
    $token = (Get-Content $tokenFile -Raw).Trim()
    $headers = @{{ "Authorization" = "Bearer $token"; "Content-Type" = "application/json" }}
    $body = @{{ name = $RemoteName }} | ConvertTo-Json -Compress
    try {{
        Invoke-RestMethod -Uri "http://$NucHost:$RemoteListenerPort/api/remote/unregister" -Method Post -Headers $headers -Body $body | Out-Null
    }} catch {{
        Write-Host "NUC-Abmeldung fehlgeschlagen (nicht kritisch)." -ForegroundColor Yellow
    }}
}}

[System.Environment]::SetEnvironmentVariable("OLLAMA_HOST", $null, "Machine")
[System.Environment]::SetEnvironmentVariable("OLLAMA_MODELS", $null, "Machine")

Write-Host "Deinstallation abgeschlossen." -ForegroundColor Green
Write-Host "Verzeichnis $InstallDir kann manuell geloescht werden." -ForegroundColor Gray
"#,
        nuc_host = spec.nuc_host,
        remote_listener_port = spec.remote_listener_port,
        remote_name = spec.remote_name,
    );
    files.push((format!("{ARCHIVE_PREFIX}/uninstall.ps1"), uninstall_ps1.into_bytes()));

    let mut checksums = String::new();
    for (name, data) in &files {
        let mut hasher = Sha256::new();
        hasher.update(data);
        let hash = hex::encode(hasher.finalize());
        let short_name = name
            .strip_prefix(&format!("{ARCHIVE_PREFIX}/"))
            .unwrap_or(name);
        checksums.push_str(&format!("{hash}  {short_name}\n"));
    }
    files.push((
        format!("{ARCHIVE_PREFIX}/checksums/SHA256SUMS.txt"),
        checksums.into_bytes(),
    ));

    for (name, data) in &files {
        zip.start_file(name, opts)?;
        zip.write_all(data)?;
    }

    let cursor = zip.finish()?;
    Ok(cursor.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;
    use zip::ZipArchive;

    fn make_test_config(token_path: &Path) -> Config {
        let config_str = format!(
            r#"
            schema_version = 1

            [gpu]
            egpu_pci_address = "0000:05:00.0"
            internal_pci_address = "0000:02:00.0"

            [local_api]
            port = 7842

            [remote]
            enabled = true
            bind = "0.0.0.0"
            port = 7843
            token_path = "{}"
            ollama_version_pin = "0.6.2"

            [[remote_gpu]]
            name = "remote-desktop"
            host = "192.168.1.55"
            port_ollama = 11435
            port_egpu_agent = 8899
            gpu_name = "RTX"
            vram_mb = 16384
            "#,
            token_path.display()
        );

        toml::from_str(&config_str).unwrap()
    }

    fn read_zip_entry(bytes: &[u8], name: &str) -> String {
        let cursor = Cursor::new(bytes.to_vec());
        let mut archive = ZipArchive::new(cursor).unwrap();
        let mut file = archive.by_name(name).unwrap();
        let mut content = String::new();
        file.read_to_string(&mut content).unwrap();
        content
    }

    #[test]
    fn test_generate_setup_package_uses_real_remote_token_and_inputs() {
        let token_path = std::env::temp_dir().join(format!(
            "egpu-remote-token-{}.secret",
            uuid::Uuid::new_v4()
        ));
        fs::write(&token_path, "expected-token\n").unwrap();

        let config = make_test_config(&token_path);
        let package = generate_setup_package(
            &config,
            SetupGenerateRequest {
                remote_name: Some("remote-desktop".to_string()),
                nuc_host: Some("10.0.0.5".to_string()),
            },
        )
        .unwrap();

        assert_eq!(package.filename, "egpu-remote-setup-remote-desktop.zip");

        let token = read_zip_entry(
            &package.zip_bytes,
            "egpu-remote-setup/config/auth-token.secret",
        );
        assert_eq!(token, "expected-token");

        let readme = read_zip_entry(&package.zip_bytes, "egpu-remote-setup/README.txt");
        assert!(readme.contains("10.0.0.5"));
        assert!(readme.contains("remote-desktop"));

        let heartbeat = read_zip_entry(
            &package.zip_bytes,
            "egpu-remote-setup/agent/heartbeat-loop.ps1",
        );
        assert!(heartbeat.contains("$NucHost = \"10.0.0.5\""));
        assert!(heartbeat.contains("$RemoteName = \"remote-desktop\""));

        let _ = fs::remove_file(token_path);
    }

    #[test]
    fn test_generate_setup_package_requires_persistent_token_path() {
        let config_str = r#"
            schema_version = 1
            [gpu]
            egpu_pci_address = "0000:05:00.0"
            internal_pci_address = "0000:02:00.0"

            [remote]
            enabled = true
            port = 7843
            token_path = ""
        "#;
        let config: Config = toml::from_str(config_str).unwrap();

        let result = generate_setup_package(&config, SetupGenerateRequest::default());
        assert!(result.is_err());
    }
}
