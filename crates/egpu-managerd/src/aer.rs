use std::collections::VecDeque;
use std::sync::Arc;
use std::time::Duration;

use egpu_manager_common::hal::AerMonitor;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, warn};

use crate::warning::WarningTrigger;

/// AER monitoring state
pub struct AerWatcher {
    pci_address: String,
    poll_interval: Duration,
    warning_threshold: u64,
    burst_threshold: u64,
    window: Duration,
    baseline: Option<u64>,
    /// (timestamp_secs_since_start, delta) pairs within the window
    recent_deltas: VecDeque<(u64, u64)>,
    elapsed_secs: u64,
}

impl AerWatcher {
    pub fn new(
        pci_address: String,
        poll_interval_seconds: u64,
        warning_threshold: u64,
        burst_threshold: u64,
        window_seconds: u64,
    ) -> Self {
        Self {
            pci_address,
            poll_interval: Duration::from_secs(poll_interval_seconds),
            warning_threshold,
            burst_threshold,
            window: Duration::from_secs(window_seconds),
            baseline: None,
            recent_deltas: VecDeque::new(),
            elapsed_secs: 0,
        }
    }

    /// Process a new AER counter reading. Returns triggers if thresholds exceeded.
    pub fn process_reading(&mut self, count: u64) -> Vec<WarningTrigger> {
        let mut triggers = Vec::new();

        match self.baseline {
            None => {
                // First reading: set baseline
                info!("AER-Baseline gesetzt: {} für {}", count, self.pci_address);
                self.baseline = Some(count);
                return triggers;
            }
            Some(baseline) => {
                if count < baseline {
                    // Counter reset detected (reboot, driver reload)
                    info!(
                        "AER-Zähler-Reset erkannt: {} < {} (Baseline) — neue Baseline für {}",
                        count, baseline, self.pci_address
                    );
                    self.baseline = Some(count);
                    self.recent_deltas.clear();
                    return triggers;
                }

                let delta = count - baseline;
                self.baseline = Some(count);

                if delta > 0 {
                    debug!(
                        "AER-Delta: +{} für {} (Gesamt: {})",
                        delta, self.pci_address, count
                    );
                }

                // Track delta in window
                self.elapsed_secs += self.poll_interval.as_secs();
                self.recent_deltas.push_back((self.elapsed_secs, delta));

                // Expire old entries outside window
                let window_start = self.elapsed_secs.saturating_sub(self.window.as_secs());
                while let Some(&(ts, _)) = self.recent_deltas.front() {
                    if ts <= window_start {
                        self.recent_deltas.pop_front();
                    } else {
                        break;
                    }
                }

                // Check burst (single interval spike)
                if delta >= self.burst_threshold {
                    warn!(
                        "AER-Burst erkannt: +{} in einem Intervall (Schwelle: {}) für {}",
                        delta, self.burst_threshold, self.pci_address
                    );
                    triggers.push(WarningTrigger::AerBurst);
                }

                // Check sustained threshold over window
                let window_total: u64 = self.recent_deltas.iter().map(|(_, d)| d).sum();
                if window_total >= self.warning_threshold && !triggers.contains(&WarningTrigger::AerBurst) {
                    warn!(
                        "AER-Schwellenwert überschritten: {} in {}s-Fenster (Schwelle: {}) für {}",
                        window_total,
                        self.window.as_secs(),
                        self.warning_threshold,
                        self.pci_address
                    );
                    triggers.push(WarningTrigger::AerThreshold);
                }
            }
        }

        triggers
    }

    /// Run the AER monitoring loop until cancelled.
    pub async fn run(
        mut self,
        monitor: Arc<dyn AerMonitor>,
        trigger_tx: mpsc::Sender<WarningTrigger>,
        cancel: CancellationToken,
    ) {
        info!(
            "AER-Monitoring gestartet für {} (Intervall: {:?})",
            self.pci_address, self.poll_interval
        );

        loop {
            tokio::select! {
                _ = cancel.cancelled() => {
                    info!("AER-Monitoring beendet für {}", self.pci_address);
                    return;
                }
                _ = tokio::time::sleep(self.poll_interval) => {
                    match monitor.read_nonfatal_count(&self.pci_address).await {
                        Ok(count) => {
                            let triggers = self.process_reading(count);
                            for trigger in triggers {
                                if trigger_tx.send(trigger).await.is_err() {
                                    error!("Trigger-Kanal geschlossen");
                                    return;
                                }
                            }
                        }
                        Err(e) => {
                            warn!("AER-Zähler nicht lesbar für {}: {e}", self.pci_address);
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
    fn test_first_reading_sets_baseline() {
        let mut watcher = AerWatcher::new("0000:05:00.0".to_string(), 5, 3, 10, 60);
        let triggers = watcher.process_reading(42);
        assert!(triggers.is_empty());
        assert_eq!(watcher.baseline, Some(42));
    }

    #[test]
    fn test_no_increase_no_trigger() {
        let mut watcher = AerWatcher::new("0000:05:00.0".to_string(), 5, 3, 10, 60);
        watcher.process_reading(0); // baseline
        let triggers = watcher.process_reading(0);
        assert!(triggers.is_empty());
    }

    #[test]
    fn test_burst_detection() {
        let mut watcher = AerWatcher::new("0000:05:00.0".to_string(), 5, 3, 10, 60);
        watcher.process_reading(0); // baseline
        let triggers = watcher.process_reading(15); // +15, burst_threshold=10
        assert!(triggers.contains(&WarningTrigger::AerBurst));
    }

    #[test]
    fn test_sustained_threshold() {
        let mut watcher = AerWatcher::new("0000:05:00.0".to_string(), 5, 3, 10, 60);
        watcher.process_reading(0); // baseline
        watcher.process_reading(1); // +1
        watcher.process_reading(2); // +1
        let triggers = watcher.process_reading(3); // +1, total=3 in window >= threshold=3
        assert!(triggers.contains(&WarningTrigger::AerThreshold));
    }

    #[test]
    fn test_counter_reset() {
        let mut watcher = AerWatcher::new("0000:05:00.0".to_string(), 5, 3, 10, 60);
        watcher.process_reading(100); // baseline
        watcher.process_reading(105); // +5
        let triggers = watcher.process_reading(2); // Reset: 2 < 105
        assert!(triggers.is_empty());
        assert_eq!(watcher.baseline, Some(2));
        assert!(watcher.recent_deltas.is_empty());
    }

    #[test]
    fn test_window_expiry() {
        // Window is 60s, poll every 5s. After 12 polls the window fills.
        let mut watcher = AerWatcher::new("0000:05:00.0".to_string(), 5, 10, 100, 60);
        watcher.process_reading(0); // baseline

        // Add 1 error per poll for 13 polls (65 seconds)
        for i in 1..=13 {
            watcher.process_reading(i);
        }

        // Window should contain only the last 12 deltas (each +1 = 12 total)
        let window_total: u64 = watcher.recent_deltas.iter().map(|(_, d)| d).sum();
        assert!(window_total <= 12, "Window total should be <= 12, got {window_total}");
    }

    #[test]
    fn test_burst_does_not_also_trigger_threshold() {
        let mut watcher = AerWatcher::new("0000:05:00.0".to_string(), 5, 3, 10, 60);
        watcher.process_reading(0);
        let triggers = watcher.process_reading(15); // burst
        // Should only contain AerBurst, not AerThreshold
        assert_eq!(triggers.len(), 1);
        assert!(triggers.contains(&WarningTrigger::AerBurst));
    }
}
