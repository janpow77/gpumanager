use std::process::Stdio;
use std::time::Duration;

use egpu_manager_common::error::GpuError;
use egpu_manager_common::gpu::*;
use tokio::process::Command;
use tracing::{debug, warn};

pub struct NvidiaSmiMonitor {
    timeout_secs: u64,
}

impl NvidiaSmiMonitor {
    pub fn new(timeout_secs: u64) -> Self {
        Self { timeout_secs }
    }

    /// Alle GPUs abfragen (nvidia-smi --query-gpu)
    pub async fn query_all(&self) -> Result<Vec<GpuStatus>, GpuError> {
        let output = self
            .run_nvidia_smi(&[
                "--query-gpu=gpu_bus_id,index,name,temperature.gpu,utilization.gpu,utilization.memory,memory.used,memory.free,memory.total,power.draw,pstate,fan.speed,clocks.current.graphics,clocks.current.memory,gpu_operation_mode.current",
                "--format=csv,noheader,nounits",
            ])
            .await?;

        parse_gpu_status_output(&output)
    }

    /// PCIe-Throughput für eine GPU abfragen (nvidia-smi dmon, einmalig)
    pub async fn query_pcie_throughput(
        &self,
        pci_address: &str,
    ) -> Result<PcieThroughput, GpuError> {
        // nvidia-smi-Index für die PCI-Adresse finden
        let gpus = self.query_all().await?;
        let gpu = gpus
            .iter()
            .find(|g| normalize_pci_address(&g.pci_address) == normalize_pci_address(pci_address))
            .ok_or_else(|| GpuError::GpuNotFound {
                pci_address: pci_address.to_string(),
            })?;

        let index = gpu.nvidia_index.ok_or_else(|| GpuError::NvidiaSmiParse(
            format!("Kein nvidia-index für {pci_address}"),
        ))?;

        let output = self
            .run_nvidia_smi(&[
                "dmon",
                "-i",
                &index.to_string(),
                "-s",
                "p",
                "-c",
                "1",
            ])
            .await?;

        parse_dmon_output(&output, pci_address)
    }

    /// Display-VRAM-Verbrauch einer GPU ermitteln
    pub async fn query_display_vram(&self, pci_address: &str) -> Result<u64, GpuError> {
        let gpus = self.query_all().await?;
        let gpu = gpus
            .iter()
            .find(|g| normalize_pci_address(&g.pci_address) == normalize_pci_address(pci_address))
            .ok_or_else(|| GpuError::GpuNotFound {
                pci_address: pci_address.to_string(),
            })?;

        // memory.used auf der internen GPU = Display-VRAM (wenn keine Container laufen)
        Ok(gpu.memory_used_mb)
    }

    async fn run_nvidia_smi(&self, args: &[&str]) -> Result<String, GpuError> {
        let timeout = Duration::from_secs(self.timeout_secs);

        let result = tokio::time::timeout(timeout, async {
            let output = Command::new("nvidia-smi")
                .args(args)
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .output()
                .await
                .map_err(|e| GpuError::NvidiaSmiUnavailable(e.to_string()))?;

            if !output.status.success() {
                let stderr = String::from_utf8_lossy(&output.stderr);
                return Err(GpuError::NvidiaSmiParse(format!(
                    "nvidia-smi Exit-Code {}: {stderr}",
                    output.status
                )));
            }

            Ok(String::from_utf8_lossy(&output.stdout).to_string())
        })
        .await;

        match result {
            Ok(inner) => inner,
            Err(_) => Err(GpuError::NvidiaSmiTimeout {
                timeout_secs: self.timeout_secs,
            }),
        }
    }
}

/// PCI-Adressen normalisieren (führende Nullen entfernen/hinzufügen)
fn normalize_pci_address(addr: &str) -> String {
    // nvidia-smi gibt manchmal "00000000:02:00.0" statt "0000:02:00.0"
    let addr = addr.trim();
    if addr.len() > 12 && addr.contains(':') {
        // Auf 4-stelligen Domain kürzen
        let parts: Vec<&str> = addr.splitn(2, ':').collect();
        if parts.len() == 2 {
            let domain = parts[0];
            if domain.len() > 4 {
                let short_domain = &domain[domain.len() - 4..];
                return format!("{short_domain}:{}", parts[1]);
            }
        }
    }
    addr.to_string()
}

fn parse_gpu_status_output(output: &str) -> Result<Vec<GpuStatus>, GpuError> {
    let mut gpus = Vec::new();

    for line in output.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        let parts: Vec<&str> = line.split(", ").collect();
        if parts.len() < 15 {
            warn!("nvidia-smi Zeile hat zu wenige Felder ({}/15): {line}", parts.len());
            continue;
        }

        let pci_address = normalize_pci_address(parts[0]);
        let nvidia_index = parts[1].trim().parse::<u32>().ok();
        let name = parts[2].trim().to_string();
        let temperature_c = parse_u32(parts[3]);
        let utilization_gpu_percent = parse_u32(parts[4]);
        let _utilization_memory_percent = parse_u32(parts[5]);
        let memory_used_mb = parse_u64(parts[6]);
        let memory_free_mb = parse_u64(parts[7]);
        let memory_total_mb = parse_u64(parts[8]);
        let power_draw_w = parts[9].trim().parse::<f64>().unwrap_or(0.0);
        let pstate = parts[10].trim().to_string();
        let fan_speed_percent = parse_u32(parts[11]);
        let clock_graphics_mhz = parse_u32(parts[12]);
        let clock_memory_mhz = parse_u32(parts[13]);
        // gpu_operation_mode als Throttle-Reason (z.B. "All On", "Compute", "[N/A]")
        let throttle_reason = parts[14].trim().to_string();

        gpus.push(GpuStatus {
            pci_address,
            nvidia_index,
            name,
            gpu_type: GpuType::Internal, // Wird vom Caller anhand der Config gesetzt
            temperature_c,
            utilization_gpu_percent,
            memory_used_mb,
            memory_free_mb,
            memory_total_mb,
            power_draw_w,
            pstate,
            fan_speed_percent,
            clock_graphics_mhz,
            clock_memory_mhz,
            throttle_reason,
            status: GpuOnlineStatus::Online,
        });
    }

    if gpus.is_empty() {
        return Err(GpuError::NvidiaSmiParse(
            "Keine GPUs in nvidia-smi-Ausgabe gefunden".to_string(),
        ));
    }

    debug!("{} GPU(s) von nvidia-smi geparst", gpus.len());
    Ok(gpus)
}

fn parse_dmon_output(output: &str, pci_address: &str) -> Result<PcieThroughput, GpuError> {
    // nvidia-smi dmon Ausgabe:
    // # gpu    pci_tx   pci_rx
    //   0      1234     5678
    for line in output.lines() {
        let line = line.trim();
        if line.starts_with('#') || line.is_empty() {
            continue;
        }

        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() >= 3 {
            let tx_kbps = parse_u64(parts[1]);
            let rx_kbps = parse_u64(parts[2]);

            return Ok(PcieThroughput {
                pci_address: pci_address.to_string(),
                tx_kbps,
                rx_kbps,
            });
        }
    }

    Err(GpuError::NvidiaSmiParse(
        "nvidia-smi dmon: keine Daten geparst".to_string(),
    ))
}

fn parse_u32(s: &str) -> u32 {
    s.trim().replace(" %", "").parse().unwrap_or(0)
}

fn parse_u64(s: &str) -> u64 {
    s.trim()
        .replace(" MiB", "")
        .replace(" MB", "")
        .parse()
        .unwrap_or(0)
}

/// Ollama-API abfragen: laufende Modelle
pub async fn query_ollama_models(host: &str) -> Result<Vec<OllamaModel>, String> {
    let url = format!("{host}/api/ps");
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .map_err(|e| format!("HTTP-Client: {e}"))?;

    let resp = client
        .get(&url)
        .send()
        .await
        .map_err(|e| format!("Ollama nicht erreichbar ({url}): {e}"))?;

    if !resp.status().is_success() {
        return Err(format!("Ollama API Fehler: HTTP {}", resp.status()));
    }

    let body: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| format!("Ollama JSON-Parse-Fehler: {e}"))?;

    let models = body["models"]
        .as_array()
        .unwrap_or(&vec![])
        .iter()
        .filter_map(|m| {
            Some(OllamaModel {
                name: m["name"].as_str()?.to_string(),
                size_bytes: m["size"].as_u64().unwrap_or(0),
                size_vram_bytes: m["size_vram"].as_u64().unwrap_or(0),
                expires_at: None,
            })
        })
        .collect();

    Ok(models)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_gpu_status() {
        let output = "00000000:02:00.0, 0, NVIDIA GeForce RTX 5060 Laptop GPU, 42, 0 %, 0 %, 15 MiB, 7692 MiB, 8151 MiB, 4.82 W, P8, 0 %, 210 MHz, 405 MHz, All On\n00000000:05:00.0, 1, NVIDIA GeForce RTX 5070 Ti, 45, 0 %, 0 %, 15788 MiB, 53 MiB, 16303 MiB, 22.73 W, P8, 30 %, 210 MHz, 405 MHz, All On\n";

        let gpus = parse_gpu_status_output(output).unwrap();
        assert_eq!(gpus.len(), 2);

        assert_eq!(gpus[0].pci_address, "0000:02:00.0");
        assert_eq!(gpus[0].nvidia_index, Some(0));
        assert_eq!(gpus[0].name, "NVIDIA GeForce RTX 5060 Laptop GPU");
        assert_eq!(gpus[0].temperature_c, 42);
        assert_eq!(gpus[0].memory_total_mb, 8151);

        assert_eq!(gpus[1].pci_address, "0000:05:00.0");
        assert_eq!(gpus[1].nvidia_index, Some(1));
        assert_eq!(gpus[1].name, "NVIDIA GeForce RTX 5070 Ti");
        assert_eq!(gpus[1].memory_total_mb, 16303);
    }

    #[test]
    fn test_parse_empty_output() {
        let result = parse_gpu_status_output("");
        assert!(result.is_err());
    }

    #[test]
    fn test_normalize_pci_address() {
        assert_eq!(normalize_pci_address("00000000:02:00.0"), "0000:02:00.0");
        assert_eq!(normalize_pci_address("0000:05:00.0"), "0000:05:00.0");
        assert_eq!(
            normalize_pci_address("  00000000:05:00.0  "),
            "0000:05:00.0"
        );
    }

    #[test]
    fn test_parse_dmon_output() {
        let output = "# gpu    pci_tx   pci_rx\n  0      45000    12000\n";
        let result = parse_dmon_output(output, "0000:05:00.0").unwrap();
        assert_eq!(result.tx_kbps, 45000);
        assert_eq!(result.rx_kbps, 12000);
    }

    #[test]
    fn test_parse_dmon_empty() {
        let result = parse_dmon_output("# header\n", "0000:05:00.0");
        assert!(result.is_err());
    }
}
