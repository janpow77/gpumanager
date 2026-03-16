use std::collections::VecDeque;
use std::time::Instant;

use serde::Serialize;
use tracing::{debug, info};

use crate::warning::WarningTrigger;

/// Types of events that affect the health score.
#[derive(Debug, Clone, Serialize)]
pub enum HealthEventKind {
    AerError,
    PcieTransient,
    NvidiaSmiSlow,
    TemperatureSpike,
    PstateAnomaly,
}

impl std::fmt::Display for HealthEventKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HealthEventKind::AerError => write!(f, "AER-Fehler"),
            HealthEventKind::PcieTransient => write!(f, "PCIe-Transient"),
            HealthEventKind::NvidiaSmiSlow => write!(f, "nvidia-smi langsam"),
            HealthEventKind::TemperatureSpike => write!(f, "Temperatur-Spike"),
            HealthEventKind::PstateAnomaly => write!(f, "P-State-Anomalie"),
        }
    }
}

/// A recorded health event.
#[derive(Debug, Clone)]
struct HealthEvent {
    kind: HealthEventKind,
    penalty: f64,
    at: Instant,
}

/// Tracks a composite health score for the eGPU link.
///
/// Score starts at 100.0 (healthy) and decreases with each negative event.
/// Recovery occurs over time when no new events arrive.
/// Hysteresis prevents trigger flapping at threshold boundaries.
pub struct LinkHealthScore {
    score: f64,
    last_update: Instant,
    event_log: VecDeque<HealthEvent>,
    warned_low: bool,
    warned_critical: bool,

    // Config
    aer_penalty: f64,
    pcie_error_penalty: f64,
    smi_slow_penalty: f64,
    thermal_penalty: f64,
    recovery_per_minute: f64,
    warning_threshold: f64,
    critical_threshold: f64,
}

const MAX_EVENT_LOG: usize = 200;
/// Hysteresis offset: trigger resets when score exceeds threshold by this amount.
const HYSTERESIS_OFFSET: f64 = 5.0;

impl LinkHealthScore {
    pub fn new(
        aer_penalty: f64,
        pcie_error_penalty: f64,
        smi_slow_penalty: f64,
        thermal_penalty: f64,
        recovery_per_minute: f64,
        warning_threshold: f64,
        critical_threshold: f64,
    ) -> Self {
        Self {
            score: 100.0,
            last_update: Instant::now(),
            event_log: VecDeque::with_capacity(MAX_EVENT_LOG),
            warned_low: false,
            warned_critical: false,
            aer_penalty,
            pcie_error_penalty,
            smi_slow_penalty,
            thermal_penalty,
            recovery_per_minute,
            warning_threshold,
            critical_threshold,
        }
    }

    /// Record a health event and apply its penalty.
    pub fn record_event(&mut self, kind: HealthEventKind) {
        let penalty = match kind {
            HealthEventKind::AerError => self.aer_penalty,
            HealthEventKind::PcieTransient => self.pcie_error_penalty,
            HealthEventKind::NvidiaSmiSlow => self.smi_slow_penalty,
            HealthEventKind::TemperatureSpike => self.thermal_penalty,
            HealthEventKind::PstateAnomaly => self.aer_penalty, // same as AER
        };

        self.score = (self.score - penalty).clamp(0.0, 100.0);
        self.last_update = Instant::now();

        debug!(
            "Health-Score: {} Event, -{} Punkte -> {:.1}",
            kind, penalty, self.score
        );

        if self.event_log.len() >= MAX_EVENT_LOG {
            self.event_log.pop_front();
        }
        self.event_log.push_back(HealthEvent {
            kind,
            penalty,
            at: Instant::now(),
        });
    }

    /// Apply time-based recovery and check thresholds.
    /// Returns a trigger if a threshold was crossed downward.
    pub fn tick(&mut self) -> Option<WarningTrigger> {
        let now = Instant::now();
        let elapsed_minutes = self.last_update.elapsed().as_secs_f64() / 60.0;

        // Only recover if there were no events in the last tick period
        if elapsed_minutes > 0.0 {
            let recovery = self.recovery_per_minute * elapsed_minutes;
            if recovery > 0.0 {
                self.score = (self.score + recovery).clamp(0.0, 100.0);
            }
        }
        self.last_update = now;

        // Check critical threshold with hysteresis
        if self.score < self.critical_threshold && !self.warned_critical {
            self.warned_critical = true;
            self.warned_low = true; // also mark low
            info!(
                "Health-Score kritisch: {:.1} < {:.1}",
                self.score, self.critical_threshold
            );
            return Some(WarningTrigger::HealthScoreCritical);
        }

        // Check warning threshold with hysteresis
        if self.score < self.warning_threshold && !self.warned_low {
            self.warned_low = true;
            info!(
                "Health-Score niedrig: {:.1} < {:.1}",
                self.score, self.warning_threshold
            );
            return Some(WarningTrigger::HealthScoreLow);
        }

        // Reset hysteresis when score recovers above threshold + offset
        if self.warned_critical && self.score > self.critical_threshold + HYSTERESIS_OFFSET {
            self.warned_critical = false;
            info!(
                "Health-Score Critical-Hysterese zurückgesetzt: {:.1}",
                self.score
            );
        }
        if self.warned_low && self.score > self.warning_threshold + HYSTERESIS_OFFSET {
            self.warned_low = false;
            info!(
                "Health-Score Warning-Hysterese zurückgesetzt: {:.1}",
                self.score
            );
        }

        None
    }

    /// Current score value.
    pub fn current_score(&self) -> f64 {
        self.score
    }

    /// JSON summary for SSE/API.
    pub fn summary(&self) -> serde_json::Value {
        let recent_events: Vec<serde_json::Value> = self
            .event_log
            .iter()
            .rev()
            .take(10)
            .map(|e| {
                serde_json::json!({
                    "kind": format!("{}", e.kind),
                    "penalty": e.penalty,
                    "seconds_ago": e.at.elapsed().as_secs(),
                })
            })
            .collect();

        serde_json::json!({
            "score": (self.score * 10.0).round() / 10.0,
            "warned_low": self.warned_low,
            "warned_critical": self.warned_critical,
            "event_count": self.event_log.len(),
            "recent_events": recent_events,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_score() -> LinkHealthScore {
        LinkHealthScore::new(3.0, 5.0, 2.0, 5.0, 1.0, 60.0, 40.0)
    }

    #[test]
    fn test_initial_score_is_100() {
        let hs = make_score();
        assert!((hs.current_score() - 100.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_aer_penalty() {
        let mut hs = make_score();
        hs.record_event(HealthEventKind::AerError);
        assert!((hs.current_score() - 97.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_pcie_penalty() {
        let mut hs = make_score();
        hs.record_event(HealthEventKind::PcieTransient);
        assert!((hs.current_score() - 95.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_smi_slow_penalty() {
        let mut hs = make_score();
        hs.record_event(HealthEventKind::NvidiaSmiSlow);
        assert!((hs.current_score() - 98.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_thermal_penalty() {
        let mut hs = make_score();
        hs.record_event(HealthEventKind::TemperatureSpike);
        assert!((hs.current_score() - 95.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_score_clamps_at_zero() {
        let mut hs = make_score();
        // 100 / 5 = 20 events to reach 0, one more to test clamping
        for _ in 0..25 {
            hs.record_event(HealthEventKind::PcieTransient);
        }
        assert!((hs.current_score() - 0.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_warning_threshold_trigger() {
        let mut hs = make_score();
        // Drop below 60: need 9 pcie events (9 * 5 = 45, 100-45=55 < 60)
        for _ in 0..9 {
            hs.record_event(HealthEventKind::PcieTransient);
        }
        let trigger = hs.tick();
        assert_eq!(trigger, Some(WarningTrigger::HealthScoreLow));
    }

    #[test]
    fn test_critical_threshold_trigger() {
        let mut hs = make_score();
        // Drop below 40: need 13 pcie events (13 * 5 = 65, 100-65=35 < 40)
        for _ in 0..13 {
            hs.record_event(HealthEventKind::PcieTransient);
        }
        let trigger = hs.tick();
        assert_eq!(trigger, Some(WarningTrigger::HealthScoreCritical));
    }

    #[test]
    fn test_hysteresis_prevents_reflap() {
        let mut hs = make_score();
        // Drop below 60
        for _ in 0..9 {
            hs.record_event(HealthEventKind::PcieTransient);
        }
        let trigger = hs.tick();
        assert_eq!(trigger, Some(WarningTrigger::HealthScoreLow));

        // Tick again without recovery — should not re-trigger
        let trigger = hs.tick();
        assert_eq!(trigger, None);
    }

    #[test]
    fn test_event_log_bounded() {
        let mut hs = make_score();
        for _ in 0..250 {
            hs.record_event(HealthEventKind::AerError);
        }
        assert!(hs.event_log.len() <= MAX_EVENT_LOG);
    }

    #[test]
    fn test_summary_json() {
        let mut hs = make_score();
        hs.record_event(HealthEventKind::AerError);
        let summary = hs.summary();
        assert!(summary.get("score").is_some());
        assert!(summary.get("event_count").is_some());
        assert_eq!(summary["event_count"], 1);
    }

    #[test]
    fn test_multiple_penalty_types() {
        let mut hs = make_score();
        hs.record_event(HealthEventKind::AerError); // -3 -> 97
        hs.record_event(HealthEventKind::PcieTransient); // -5 -> 92
        hs.record_event(HealthEventKind::NvidiaSmiSlow); // -2 -> 90
        hs.record_event(HealthEventKind::TemperatureSpike); // -5 -> 85
        assert!((hs.current_score() - 85.0).abs() < f64::EPSILON);
    }
}
