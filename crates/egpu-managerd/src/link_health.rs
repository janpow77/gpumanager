use std::sync::Arc;
use std::time::Duration;

use egpu_manager_common::gpu::PcieLinkHealth;
use egpu_manager_common::hal::PcieLinkMonitor;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, warn};

use crate::warning::WarningTrigger;

/// Thunderbolt-expected link parameters.
const TB_EXPECTED_WIDTH: u8 = 4;
const TB_EXPECTED_SPEED: &str = "2.5 GT/s";

/// PCIe link health monitoring loop.
pub struct LinkHealthWatcher {
    pci_address: String,
    poll_interval: Duration,
}

impl LinkHealthWatcher {
    pub fn new(pci_address: String, poll_interval_ms: u64) -> Self {
        Self {
            pci_address,
            poll_interval: Duration::from_millis(poll_interval_ms),
        }
    }

    /// Evaluate a single link health reading and return applicable triggers.
    pub fn evaluate(health: &PcieLinkHealth) -> Vec<WarningTrigger> {
        let mut triggers = Vec::new();

        if health.is_link_down() {
            warn!(
                "PCIe-Link DOWN für {}: speed={}, width={}",
                health.pci_address, health.current_link_speed, health.current_link_width
            );
            triggers.push(WarningTrigger::LinkDown);
            return triggers;
        }

        // Check width degradation against Thunderbolt expectation
        if health.current_link_width < TB_EXPECTED_WIDTH {
            warn!(
                "PCIe-Link-Breite degradiert für {}: x{} (erwartet: x{})",
                health.pci_address, health.current_link_width, TB_EXPECTED_WIDTH
            );
            triggers.push(WarningTrigger::LinkWidthDegradation);
        }

        // Check speed degradation against Thunderbolt expectation
        if health.current_link_speed != TB_EXPECTED_SPEED
            && health.current_link_speed != "Unknown"
        {
            // Only warn if speed is lower, not higher
            // Parse GT/s value for comparison
            let current_gts = parse_gts(&health.current_link_speed);
            let expected_gts = parse_gts(TB_EXPECTED_SPEED);

            if let (Some(current), Some(expected)) = (current_gts, expected_gts)
                && current < expected
            {
                warn!(
                    "PCIe-Link-Geschwindigkeit degradiert für {}: {} (erwartet: {})",
                    health.pci_address, health.current_link_speed, TB_EXPECTED_SPEED
                );
                triggers.push(WarningTrigger::LinkSpeedDegradation);
            }
        }

        triggers
    }

    /// Run the link health monitoring loop until cancelled.
    pub async fn run(
        self,
        monitor: Arc<dyn PcieLinkMonitor>,
        trigger_tx: mpsc::Sender<WarningTrigger>,
        cancel: CancellationToken,
    ) {
        info!(
            "Link-Health-Monitoring gestartet für {} (Intervall: {:?})",
            self.pci_address, self.poll_interval
        );

        loop {
            tokio::select! {
                _ = cancel.cancelled() => {
                    info!("Link-Health-Monitoring beendet für {}", self.pci_address);
                    return;
                }
                _ = tokio::time::sleep(self.poll_interval) => {
                    match monitor.read_link_health(&self.pci_address).await {
                        Ok(health) => {
                            let triggers = Self::evaluate(&health);
                            if triggers.is_empty() {
                                debug!(
                                    "Link-Health OK: {} x{} für {}",
                                    health.current_link_speed,
                                    health.current_link_width,
                                    self.pci_address
                                );
                            }
                            for trigger in triggers {
                                if trigger_tx.send(trigger).await.is_err() {
                                    error!("Trigger-Kanal geschlossen");
                                    return;
                                }
                            }
                        }
                        Err(e) => {
                            warn!(
                                "Link-Health nicht lesbar für {}: {e}",
                                self.pci_address
                            );
                        }
                    }
                }
            }
        }
    }
}

/// Parse a speed string like "2.5 GT/s" or "8.0 GT/s" into a float.
fn parse_gts(s: &str) -> Option<f64> {
    let s = s.trim();
    let num_str = s.split_whitespace().next()?;
    num_str.parse::<f64>().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_health(speed: &str, width: u8) -> PcieLinkHealth {
        PcieLinkHealth {
            pci_address: "0000:05:00.0".to_string(),
            current_link_speed: speed.to_string(),
            current_link_width: width,
            max_link_speed: "32.0 GT/s".to_string(),
            max_link_width: 16,
            degraded: false,
        }
    }

    #[test]
    fn test_healthy_link() {
        let health = make_health("2.5 GT/s", 4);
        let triggers = LinkHealthWatcher::evaluate(&health);
        assert!(triggers.is_empty());
    }

    #[test]
    fn test_width_degradation() {
        let health = make_health("2.5 GT/s", 2);
        let triggers = LinkHealthWatcher::evaluate(&health);
        assert!(triggers.contains(&WarningTrigger::LinkWidthDegradation));
    }

    #[test]
    fn test_link_down_unknown() {
        let health = make_health("Unknown", 0);
        let triggers = LinkHealthWatcher::evaluate(&health);
        assert!(triggers.contains(&WarningTrigger::LinkDown));
        // LinkDown should not also trigger width/speed degradation
        assert!(!triggers.contains(&WarningTrigger::LinkWidthDegradation));
    }

    #[test]
    fn test_link_down_zero_width() {
        let health = make_health("2.5 GT/s", 0);
        let triggers = LinkHealthWatcher::evaluate(&health);
        assert!(triggers.contains(&WarningTrigger::LinkDown));
    }

    #[test]
    fn test_higher_speed_no_degradation() {
        // If speed is higher than expected, that is fine (not degraded)
        let health = make_health("8.0 GT/s", 4);
        let triggers = LinkHealthWatcher::evaluate(&health);
        assert!(triggers.is_empty());
    }

    #[test]
    fn test_parse_gts() {
        assert_eq!(parse_gts("2.5 GT/s"), Some(2.5));
        assert_eq!(parse_gts("8.0 GT/s"), Some(8.0));
        assert_eq!(parse_gts("32.0 GT/s"), Some(32.0));
        assert_eq!(parse_gts("Unknown"), None);
    }

    #[test]
    fn test_both_degradations() {
        let health = make_health("1.0 GT/s", 2);
        let triggers = LinkHealthWatcher::evaluate(&health);
        assert!(triggers.contains(&WarningTrigger::LinkWidthDegradation));
        assert!(triggers.contains(&WarningTrigger::LinkSpeedDegradation));
    }
}
