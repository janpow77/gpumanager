use egpu_manager_common::error::{AerError, PcieError};
use egpu_manager_common::gpu::PcieLinkHealth;
use egpu_manager_common::hal::{AerMonitor, PcieLinkMonitor};

use async_trait::async_trait;
use tracing::debug;

/// Liest PCIe-Link-Health aus sysfs
pub struct SysfsLinkMonitor;

#[async_trait]
impl PcieLinkMonitor for SysfsLinkMonitor {
    async fn read_link_health(&self, pci_address: &str) -> Result<PcieLinkHealth, PcieError> {
        let base = format!("/sys/bus/pci/devices/{pci_address}");

        let current_speed = read_sysfs_trimmed(&format!("{base}/current_link_speed"))
            .map_err(|e| PcieError::LinkReadError {
                pci_address: pci_address.to_string(),
                reason: format!("current_link_speed: {e}"),
            })?;

        let current_width = read_sysfs_u8(&format!("{base}/current_link_width"))
            .map_err(|e| PcieError::LinkReadError {
                pci_address: pci_address.to_string(),
                reason: format!("current_link_width: {e}"),
            })?;

        let max_speed = read_sysfs_trimmed(&format!("{base}/max_link_speed"))
            .map_err(|e| PcieError::LinkReadError {
                pci_address: pci_address.to_string(),
                reason: format!("max_link_speed: {e}"),
            })?;

        let max_width = read_sysfs_u8(&format!("{base}/max_link_width"))
            .map_err(|e| PcieError::LinkReadError {
                pci_address: pci_address.to_string(),
                reason: format!("max_link_width: {e}"),
            })?;

        let health = PcieLinkHealth {
            pci_address: pci_address.to_string(),
            current_link_speed: current_speed,
            current_link_width: current_width,
            max_link_speed: max_speed,
            max_link_width: max_width,
            degraded: false, // Will be computed by caller via is_degraded()
        };

        debug!(
            "Link-Health {pci_address}: {} x{} (max: {} x{})",
            health.current_link_speed,
            health.current_link_width,
            health.max_link_speed,
            health.max_link_width
        );

        Ok(health)
    }
}

/// Liest AER Non-Fatal-Fehlerzähler aus sysfs
pub struct SysfsAerMonitor;

#[async_trait]
impl AerMonitor for SysfsAerMonitor {
    async fn read_nonfatal_count(&self, pci_address: &str) -> Result<u64, AerError> {
        let path = format!("/sys/bus/pci/devices/{pci_address}/aer_dev_nonfatal");
        let content =
            read_sysfs_trimmed(&path).map_err(|e| AerError::ReadError(format!("{path}: {e}")))?;

        // Prüfen ob es ein Multi-Line-Format ist (wie bei neueren Kernels)
        if content.contains('\n') || content.contains("TOTAL_ERR_NONFATAL") {
            parse_aer_nonfatal(&content).map_err(|e| AerError::ReadError(format!("{path}: {e}")))
        } else {
            // Einfacher Zahlenwert
            read_sysfs_u64(&path).map_err(|e| AerError::ReadError(format!("{path}: {e}")))
        }
    }
}

fn read_sysfs_trimmed(path: &str) -> Result<String, String> {
    std::fs::read_to_string(path)
        .map(|s| s.trim().to_string())
        .map_err(|e| format!("{e}"))
}

fn read_sysfs_u8(path: &str) -> Result<u8, String> {
    let content = read_sysfs_trimmed(path)?;
    content.parse::<u8>().map_err(|e| format!("Parse '{content}': {e}"))
}

fn read_sysfs_u64(path: &str) -> Result<u64, String> {
    let content = read_sysfs_trimmed(path)?;
    // AER-Zähler können hex oder dezimal sein
    if let Some(hex) = content.strip_prefix("0x") {
        u64::from_str_radix(hex, 16).map_err(|e| format!("Parse hex '{content}': {e}"))
    } else {
        content.parse::<u64>().map_err(|e| format!("Parse '{content}': {e}"))
    }
}

/// AER Non-Fatal-Zähler: Multi-Line-Format parsen
/// Format:
/// ```text
/// Undefined 0
/// DLP 0
/// ...
/// TOTAL_ERR_NONFATAL 0
/// ```
fn parse_aer_nonfatal(content: &str) -> Result<u64, String> {
    for line in content.lines() {
        let line = line.trim();
        if line.starts_with("TOTAL_ERR_NONFATAL") {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() >= 2 {
                return parts[1]
                    .parse::<u64>()
                    .map_err(|e| format!("Parse TOTAL_ERR_NONFATAL '{line}': {e}"));
            }
        }
    }
    // Fallback: Wenn TOTAL nicht vorhanden, CmpltTO direkt lesen
    for line in content.lines() {
        let line = line.trim();
        if line.starts_with("CmpltTO") {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() >= 2 {
                return parts[1]
                    .parse::<u64>()
                    .map_err(|e| format!("Parse CmpltTO '{line}': {e}"));
            }
        }
    }
    Err("Weder TOTAL_ERR_NONFATAL noch CmpltTO in AER-Ausgabe gefunden".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pcie_link_health_degraded() {
        let health = PcieLinkHealth {
            pci_address: "0000:05:00.0".to_string(),
            current_link_speed: "2.5 GT/s".to_string(),
            current_link_width: 1,
            max_link_speed: "2.5 GT/s".to_string(),
            max_link_width: 4,
            degraded: false,
        };
        assert!(health.is_degraded());
        assert!(!health.is_speed_degraded());
        assert!(!health.is_link_down());
    }

    #[test]
    fn test_pcie_link_health_ok() {
        let health = PcieLinkHealth {
            pci_address: "0000:05:00.0".to_string(),
            current_link_speed: "2.5 GT/s".to_string(),
            current_link_width: 4,
            max_link_speed: "2.5 GT/s".to_string(),
            max_link_width: 4,
            degraded: false,
        };
        assert!(!health.is_degraded());
        assert!(!health.is_speed_degraded());
    }

    #[test]
    fn test_parse_aer_nonfatal_multiline() {
        let content = "Undefined 0\nDLP 0\nSDES 0\nTLP 0\nFCP 0\nCmpltTO 0\nCmpltAbrt 0\nUnxCmplt 0\nRxOF 0\nMalfTLP 0\nECRC 0\nUnsupReq 0\nACSViol 0\nUncorrIntErr 0\nBlockedTLP 0\nAtomicOpBlocked 0\nTLPBlockedErr 0\nPoisonTLPBlocked 0\nDMWrReqBlocked 0\nIDECheck 0\nMisIDETLP 0\nPCRC_CHECK 0\nTLPXlatBlocked 0\nTOTAL_ERR_NONFATAL 0\n";
        let count = parse_aer_nonfatal(content).unwrap();
        assert_eq!(count, 0);
    }

    #[test]
    fn test_parse_aer_nonfatal_with_errors() {
        let content = "CmpltTO 5\nTOTAL_ERR_NONFATAL 7\n";
        let count = parse_aer_nonfatal(content).unwrap();
        assert_eq!(count, 7);
    }

    #[test]
    fn test_parse_aer_nonfatal_cmplto_fallback() {
        let content = "CmpltTO 3\n";
        let count = parse_aer_nonfatal(content).unwrap();
        assert_eq!(count, 3);
    }

    #[test]
    fn test_pcie_link_down() {
        let health = PcieLinkHealth {
            pci_address: "0000:05:00.0".to_string(),
            current_link_speed: "Unknown".to_string(),
            current_link_width: 0,
            max_link_speed: "2.5 GT/s".to_string(),
            max_link_width: 4,
            degraded: false,
        };
        assert!(health.is_link_down());
    }
}
