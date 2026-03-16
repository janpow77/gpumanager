use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, warn};

use crate::warning::WarningTrigger;

/// Monitors /dev/kmsg for CmpltTO patterns and nvidia-modeset GPU progress errors
/// associated with the eGPU PCI address.
pub struct KmsgMonitor {
    pci_address: String,
    /// The GPU index (e.g. "1" for GPU:1) derived from the PCI topology.
    /// Used to match nvidia-modeset errors like "GPU:1: Error while waiting for GPU progress".
    gpu_index: Option<String>,
    kmsg_path: String,
}

impl KmsgMonitor {
    pub fn new(pci_address: String) -> Self {
        let gpu_index = Self::detect_gpu_index(&pci_address);
        Self {
            pci_address,
            gpu_index,
            kmsg_path: "/dev/kmsg".to_string(),
        }
    }

    #[cfg(test)]
    #[allow(dead_code)]
    pub fn with_path(pci_address: String, path: String) -> Self {
        let gpu_index = Self::detect_gpu_index(&pci_address);
        Self {
            pci_address,
            gpu_index,
            kmsg_path: path,
        }
    }

    /// Detect the GPU index by checking nvidia-smi for the PCI address mapping.
    /// Falls back to None if detection fails (will match any GPU:N index).
    fn detect_gpu_index(pci_address: &str) -> Option<String> {
        // Try to read GPU index from nvidia-smi at startup.
        // Format: "GPU 0: NVIDIA GeForce RTX 5090 (UUID: ...)" with bus ID mapping.
        // We use a simpler approach: check /proc/driver/nvidia/gpus/ directory.
        let nvidia_gpus_dir = std::path::Path::new("/proc/driver/nvidia/gpus");
        if !nvidia_gpus_dir.exists() {
            return None;
        }

        // nvidia uses the PCI address as directory name (lowercase, e.g. "0000:05:00.0")
        let pci_lower = pci_address.to_lowercase();
        if let Ok(entries) = std::fs::read_dir(nvidia_gpus_dir) {
            for (idx, entry) in entries.flatten().enumerate() {
                let name = entry.file_name().to_string_lossy().to_lowercase();
                if name == pci_lower {
                    return Some(idx.to_string());
                }
            }
        }
        None
    }

    /// Check if a kernel log line matches a CmpltTO pattern for our PCI address.
    pub fn matches_cmplto(&self, line: &str) -> bool {
        let has_cmplto = line.contains("CmpltTO");
        let has_pci = self.line_matches_pci(line);
        has_cmplto && has_pci
    }

    /// Check if a kernel log line matches an nvidia-modeset GPU progress error.
    /// Pattern: "nvidia-modeset: ERROR: GPU:N: Error while waiting for GPU progress"
    /// This is the exact error that caused the crash on 2026-03-16 at 06:35.
    pub fn matches_gpu_progress_error(&self, line: &str) -> bool {
        if !line.contains("nvidia-modeset") || !line.contains("Error while waiting for GPU progress") {
            return false;
        }

        // Match GPU:N where N is our GPU index
        match &self.gpu_index {
            Some(idx) => {
                let pattern = format!("GPU:{idx}:");
                line.contains(&pattern)
            }
            // If we don't know our GPU index, match any GPU progress error
            None => true,
        }
    }

    /// Check if a kernel log line matches our PCI address (full or short form).
    fn line_matches_pci(&self, line: &str) -> bool {
        if line.contains(&self.pci_address) {
            return true;
        }

        // Also check for shortened PCI address form (e.g., "05:00.0")
        if let Some(idx) = self.pci_address.rfind(':') {
            if idx > 2 {
                let prefix_end = self.pci_address[..idx].rfind(':').unwrap_or(0);
                let short_pci = &self.pci_address[prefix_end..];
                return line.contains(short_pci);
            }
        }

        false
    }

    /// Run the kmsg monitoring loop until cancelled.
    pub async fn run(
        self,
        trigger_tx: mpsc::Sender<WarningTrigger>,
        cancel: CancellationToken,
    ) {
        info!(
            "Kmsg-Monitoring gestartet für PCI {} ({})",
            self.pci_address, self.kmsg_path
        );

        let file = match tokio::fs::File::open(&self.kmsg_path).await {
            Ok(f) => f,
            Err(e) => {
                warn!(
                    "Kann {} nicht öffnen: {e} — Kmsg-Monitoring deaktiviert",
                    self.kmsg_path
                );
                return;
            }
        };

        let reader = BufReader::new(file);
        let mut lines = reader.lines();

        // Drain all existing (old) messages before monitoring for new ones.
        // /dev/kmsg delivers the entire kernel ring buffer on open.
        // We only want to react to messages that appear AFTER the daemon starts.
        let mut drained = 0u32;
        loop {
            match tokio::time::timeout(
                std::time::Duration::from_millis(200),
                lines.next_line(),
            )
            .await
            {
                Ok(Ok(Some(_))) => {
                    drained += 1;
                }
                _ => break, // Timeout = no more buffered messages, or error
            }
        }
        if drained > 0 {
            info!(
                "Kmsg: {drained} bestehende Kernel-Messages übersprungen (nur neue werden überwacht)"
            );
        }

        loop {
            tokio::select! {
                _ = cancel.cancelled() => {
                    info!("Kmsg-Monitoring beendet");
                    return;
                }
                result = lines.next_line() => {
                    match result {
                        Ok(Some(line)) => {
                            if self.matches_cmplto(&line) {
                                warn!("CmpltTO in Kernel-Log erkannt: {}", line);
                                if trigger_tx.send(WarningTrigger::CmpltToPattern).await.is_err() {
                                    error!("Trigger-Kanal geschlossen");
                                    return;
                                }
                            } else if self.matches_gpu_progress_error(&line) {
                                error!("GPU-Progress-Error in Kernel-Log erkannt: {}", line);
                                if trigger_tx.send(WarningTrigger::GpuProgressError).await.is_err() {
                                    error!("Trigger-Kanal geschlossen");
                                    return;
                                }
                            }
                        }
                        Ok(None) => {
                            // EOF — /dev/kmsg should not normally EOF, but handle gracefully
                            debug!("Kmsg EOF — warte auf neue Nachrichten");
                            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                        }
                        Err(e) => {
                            // /dev/kmsg can return EAGAIN or EPIPE errors
                            debug!("Kmsg Lese-Fehler (normal bei /dev/kmsg): {e}");
                            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
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

    #[test]
    fn test_matches_cmplto_with_full_address() {
        let monitor = KmsgMonitor::new("0000:05:00.0".to_string());

        assert!(monitor.matches_cmplto(
            "pcieport 0000:00:01.0: AER: Corrected error received: 0000:05:00.0 CmpltTO"
        ));
    }

    #[test]
    fn test_matches_cmplto_with_nvidia_prefix() {
        let monitor = KmsgMonitor::new("0000:05:00.0".to_string());

        assert!(monitor
            .matches_cmplto("nvidia 0000:05:00.0: PCIe AER Non-Fatal Error: CmpltTO detected"));
    }

    #[test]
    fn test_no_match_different_address() {
        let monitor = KmsgMonitor::new("0000:05:00.0".to_string());

        assert!(!monitor.matches_cmplto(
            "pcieport 0000:00:01.0: AER: Corrected error received: 0000:02:00.0 CmpltTO"
        ));
    }

    #[test]
    fn test_no_match_without_cmplto() {
        let monitor = KmsgMonitor::new("0000:05:00.0".to_string());

        assert!(!monitor.matches_cmplto(
            "nvidia 0000:05:00.0: GPU has fallen off the bus"
        ));
    }

    #[test]
    fn test_no_match_empty_line() {
        let monitor = KmsgMonitor::new("0000:05:00.0".to_string());
        assert!(!monitor.matches_cmplto(""));
    }

    #[test]
    fn test_matches_short_pci_form() {
        let monitor = KmsgMonitor::new("0000:05:00.0".to_string());
        // Some kernel messages might use shorter form
        assert!(monitor.matches_cmplto("AER error :05:00.0 CmpltTO"));
    }

    #[test]
    fn test_matches_gpu_progress_error() {
        let mut monitor = KmsgMonitor::new("0000:05:00.0".to_string());
        // Set GPU index to 1 (eGPU is typically GPU:1)
        monitor.gpu_index = Some("1".to_string());

        assert!(monitor.matches_gpu_progress_error(
            "nvidia-modeset: ERROR: GPU:1: Error while waiting for GPU progress: 0x0000ca7d:0 2:0:4048:4040"
        ));
    }

    #[test]
    fn test_gpu_progress_error_wrong_gpu_index() {
        let mut monitor = KmsgMonitor::new("0000:05:00.0".to_string());
        monitor.gpu_index = Some("1".to_string());

        // GPU:0 should not match when our index is 1
        assert!(!monitor.matches_gpu_progress_error(
            "nvidia-modeset: ERROR: GPU:0: Error while waiting for GPU progress: 0x0000ca7d:0"
        ));
    }

    #[test]
    fn test_gpu_progress_error_no_index_matches_any() {
        let mut monitor = KmsgMonitor::new("0000:05:00.0".to_string());
        monitor.gpu_index = None; // Unknown index

        // Should match any GPU:N when index is unknown
        assert!(monitor.matches_gpu_progress_error(
            "nvidia-modeset: ERROR: GPU:1: Error while waiting for GPU progress: 0x0000ca7d:0"
        ));
    }

    #[test]
    fn test_gpu_progress_error_no_match_other_nvidia_errors() {
        let mut monitor = KmsgMonitor::new("0000:05:00.0".to_string());
        monitor.gpu_index = Some("1".to_string());

        // Other nvidia-modeset errors should NOT match
        assert!(!monitor.matches_gpu_progress_error(
            "nvidia-modeset: ERROR: GPU:1: Idling display engine timed out"
        ));
    }
}
