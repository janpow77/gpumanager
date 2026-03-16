use std::collections::VecDeque;
use std::time::{Duration, Instant};

use egpu_manager_common::gpu::WarningLevel;
use serde::{Deserialize, Serialize};
use tracing::{info, warn};

/// Triggers that can cause warning level transitions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum WarningTrigger {
    /// AER error count exceeded sustained threshold over window
    AerThreshold,
    /// AER burst: large delta in a single interval
    AerBurst,
    /// PCIe link width degradation (below Thunderbolt expectation)
    LinkWidthDegradation,
    /// PCIe link speed degradation
    LinkSpeedDegradation,
    /// PCIe link completely down
    LinkDown,
    /// nvidia-smi timed out consecutively
    NvidiaSmiTimeout,
    /// CmpltTO pattern detected in kernel log
    CmpltToPattern,
    /// CUDA watchdog timeout
    CudaWatchdogTimeout,
    /// nvidia-modeset GPU progress error — GPU hang imminent, highest severity
    GpuProgressError,
    /// GPU temperature reached throttle threshold
    ThermalThrottle,
    /// GPU temperature reached critical threshold
    ThermalCritical,
    /// P-State at P4+ sustained too long
    PstateThrottle,
    /// nvidia-smi response average too slow
    NvidiaSmiSlow,
    /// Health score dropped below warning threshold
    HealthScoreLow,
    /// Health score dropped below critical threshold
    HealthScoreCritical,
    /// All-clear: no issues detected
    AllClear,
}

impl std::fmt::Display for WarningTrigger {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            WarningTrigger::AerThreshold => write!(f, "AER-Schwellenwert überschritten"),
            WarningTrigger::AerBurst => write!(f, "AER-Burst erkannt"),
            WarningTrigger::LinkWidthDegradation => write!(f, "PCIe-Link-Breite degradiert"),
            WarningTrigger::LinkSpeedDegradation => write!(f, "PCIe-Link-Geschwindigkeit degradiert"),
            WarningTrigger::LinkDown => write!(f, "PCIe-Link ausgefallen"),
            WarningTrigger::NvidiaSmiTimeout => write!(f, "nvidia-smi Timeout"),
            WarningTrigger::CmpltToPattern => write!(f, "CmpltTO in Kernel-Log"),
            WarningTrigger::CudaWatchdogTimeout => write!(f, "CUDA-Watchdog Timeout"),
            WarningTrigger::GpuProgressError => write!(f, "GPU-Progress-Error (nvidia-modeset Hang)"),
            WarningTrigger::ThermalThrottle => write!(f, "GPU-Temperatur Throttle-Schwelle erreicht"),
            WarningTrigger::ThermalCritical => write!(f, "GPU-Temperatur kritisch"),
            WarningTrigger::PstateThrottle => write!(f, "P-State Throttling erkannt"),
            WarningTrigger::NvidiaSmiSlow => write!(f, "nvidia-smi Antwortzeit zu hoch"),
            WarningTrigger::HealthScoreLow => write!(f, "Health-Score niedrig"),
            WarningTrigger::HealthScoreCritical => write!(f, "Health-Score kritisch"),
            WarningTrigger::AllClear => write!(f, "Alle Prüfungen OK"),
        }
    }
}

impl WarningTrigger {
    /// Determine the minimum warning level this trigger should cause.
    pub fn target_level(&self) -> WarningLevel {
        match self {
            WarningTrigger::AerThreshold => WarningLevel::Yellow,
            WarningTrigger::AerBurst => WarningLevel::Orange,
            WarningTrigger::LinkWidthDegradation => WarningLevel::Orange,
            WarningTrigger::LinkSpeedDegradation => WarningLevel::Yellow,
            WarningTrigger::LinkDown => WarningLevel::Orange,
            WarningTrigger::NvidiaSmiTimeout => WarningLevel::Orange,
            WarningTrigger::CmpltToPattern => WarningLevel::Orange,
            WarningTrigger::CudaWatchdogTimeout => WarningLevel::Orange,
            WarningTrigger::GpuProgressError => WarningLevel::Red,
            WarningTrigger::ThermalThrottle => WarningLevel::Yellow,
            WarningTrigger::ThermalCritical => WarningLevel::Orange,
            WarningTrigger::PstateThrottle => WarningLevel::Yellow,
            WarningTrigger::NvidiaSmiSlow => WarningLevel::Yellow,
            WarningTrigger::HealthScoreLow => WarningLevel::Yellow,
            WarningTrigger::HealthScoreCritical => WarningLevel::Orange,
            WarningTrigger::AllClear => WarningLevel::Green,
        }
    }
}

/// Describes a pipeline that should be considered for migration.
#[derive(Debug, Clone)]
pub struct MigrationAction {
    pub pipeline_name: String,
    pub priority: u32,
    pub action: MigrationActionType,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(dead_code)]
pub enum MigrationActionType {
    /// Block new tasks from being scheduled on eGPU
    BlockNewTasks,
    /// Migrate workload away from eGPU
    Migrate,
    /// Re-migrate workload back to eGPU
    ReMigrate,
}

/// A recorded transition for history/debugging.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct Transition {
    pub from: WarningLevel,
    pub to: WarningLevel,
    pub trigger: WarningTrigger,
    pub at: Instant,
}

/// The warning level state machine.
///
/// Manages transitions between Green/Yellow/Orange/Red based on incoming
/// triggers. Implements hysteresis via a cooldown timer: after the last
/// trigger cleared, the machine waits `cooldown` before stepping down.
pub struct WarningStateMachine {
    level: WarningLevel,
    cooldown: Duration,
    /// When the last escalation trigger was received. Used for hysteresis:
    /// we only step down after cooldown has elapsed since the last trigger.
    last_trigger_time: Option<Instant>,
    /// Active triggers (non-AllClear) currently holding the level up.
    active_triggers: Vec<WarningTrigger>,
    /// History of transitions (bounded).
    history: VecDeque<Transition>,
    max_history: usize,
}

impl WarningStateMachine {
    pub fn new(cooldown_seconds: u64) -> Self {
        Self {
            level: WarningLevel::Green,
            cooldown: Duration::from_secs(cooldown_seconds),
            last_trigger_time: None,
            active_triggers: Vec::new(),
            history: VecDeque::new(),
            max_history: 100,
        }
    }

    pub fn current_level(&self) -> WarningLevel {
        self.level
    }

    #[allow(dead_code)]
    pub fn active_triggers(&self) -> &[WarningTrigger] {
        &self.active_triggers
    }

    #[allow(dead_code)]
    pub fn history(&self) -> &VecDeque<Transition> {
        &self.history
    }

    /// Process a trigger. Returns `Some(new_level)` if the level changed,
    /// `None` if it stayed the same.
    pub fn process_trigger(&mut self, trigger: WarningTrigger) -> Option<WarningLevel> {
        let now = Instant::now();

        if trigger == WarningTrigger::AllClear {
            return self.handle_all_clear(now);
        }

        // Add to active triggers if not already present
        if !self.active_triggers.contains(&trigger) {
            self.active_triggers.push(trigger.clone());
        }
        self.last_trigger_time = Some(now);

        let target = self.compute_target_level();

        if target > self.level {
            let old = self.level;
            self.level = target;
            self.record_transition(old, target, trigger, now);
            warn!(
                "Warnstufe erhöht: {} -> {} (Auslöser: {})",
                old, self.level, self.active_triggers.last().unwrap()
            );
            Some(self.level)
        } else {
            None
        }
    }

    /// Clear a specific trigger type. Returns `Some(new_level)` if the level
    /// stepped down as a result.
    #[allow(dead_code)]
    pub fn clear_trigger(&mut self, trigger: &WarningTrigger) -> Option<WarningLevel> {
        self.active_triggers.retain(|t| t != trigger);

        if self.active_triggers.is_empty() {
            return self.handle_all_clear(Instant::now());
        }

        // Recompute target from remaining triggers
        let target = self.compute_target_level();
        if target < self.level {
            // Check hysteresis cooldown
            if let Some(last) = self.last_trigger_time
                && last.elapsed() < self.cooldown
            {
                // Not enough time has passed, hold current level
                return None;
            }
            let old = self.level;
            self.level = target;
            self.record_transition(
                old,
                target,
                WarningTrigger::AllClear,
                Instant::now(),
            );
            info!("Warnstufe gesenkt: {} -> {}", old, self.level);
            Some(self.level)
        } else {
            None
        }
    }

    /// Try to step down if cooldown has elapsed and no triggers are active.
    /// Should be called periodically by the monitor loop.
    pub fn try_step_down(&mut self) -> Option<WarningLevel> {
        if self.level == WarningLevel::Green {
            return None;
        }

        if !self.active_triggers.is_empty() {
            return None;
        }

        if let Some(last) = self.last_trigger_time
            && last.elapsed() < self.cooldown
        {
            return None;
        }

        // Step down one level at a time
        let old = self.level;
        self.level = match self.level {
            WarningLevel::Red => WarningLevel::Orange,
            WarningLevel::Orange => WarningLevel::Yellow,
            WarningLevel::Yellow => WarningLevel::Green,
            WarningLevel::Green => WarningLevel::Green,
        };

        if self.level != old {
            // Reset last_trigger_time so next step-down also requires cooldown
            self.last_trigger_time = Some(Instant::now());
            self.record_transition(
                old,
                self.level,
                WarningTrigger::AllClear,
                Instant::now(),
            );
            info!("Warnstufe gesenkt (Cooldown abgelaufen): {} -> {}", old, self.level);
            Some(self.level)
        } else {
            None
        }
    }

    /// Get migration actions for the current warning level.
    /// `pipelines` is a list of (name, priority) tuples for pipelines on the eGPU.
    pub fn migration_actions(
        &self,
        pipelines_on_egpu: &[(String, u32)],
    ) -> Vec<MigrationAction> {
        let mut actions = Vec::new();

        match self.level {
            WarningLevel::Green => {
                // No actions needed
            }
            WarningLevel::Yellow => {
                // Block new tasks on eGPU
                actions.push(MigrationAction {
                    pipeline_name: "*".to_string(),
                    priority: 0,
                    action: MigrationActionType::BlockNewTasks,
                });
                // Migrate priority 4-5 pipelines away
                for (name, prio) in pipelines_on_egpu {
                    if *prio >= 4 {
                        actions.push(MigrationAction {
                            pipeline_name: name.clone(),
                            priority: *prio,
                            action: MigrationActionType::Migrate,
                        });
                    }
                }
            }
            WarningLevel::Orange => {
                // Migrate all pipelines from eGPU
                for (name, prio) in pipelines_on_egpu {
                    actions.push(MigrationAction {
                        pipeline_name: name.clone(),
                        priority: *prio,
                        action: MigrationActionType::Migrate,
                    });
                }
            }
            WarningLevel::Red => {
                // Migrate all pipelines from eGPU (emergency)
                for (name, prio) in pipelines_on_egpu {
                    actions.push(MigrationAction {
                        pipeline_name: name.clone(),
                        priority: *prio,
                        action: MigrationActionType::Migrate,
                    });
                }
            }
        }

        actions
    }

    /// Get re-migration actions when transitioning from Yellow to Green.
    /// Returns pipelines sorted by priority (lowest first) for step-by-step
    /// re-migration with 30s gaps between each.
    #[allow(dead_code)]
    pub fn remigration_order(
        &self,
        pipelines_migrated: &[(String, u32)],
    ) -> Vec<MigrationAction> {
        if self.level != WarningLevel::Green {
            return Vec::new();
        }

        let mut sorted: Vec<_> = pipelines_migrated.to_vec();
        sorted.sort_by_key(|(_, p)| *p);

        sorted
            .into_iter()
            .map(|(name, prio)| MigrationAction {
                pipeline_name: name,
                priority: prio,
                action: MigrationActionType::ReMigrate,
            })
            .collect()
    }

    fn handle_all_clear(&mut self, now: Instant) -> Option<WarningLevel> {
        self.active_triggers.clear();

        if self.level == WarningLevel::Green {
            return None;
        }

        // Check hysteresis
        if let Some(last) = self.last_trigger_time
            && last.elapsed() < self.cooldown
        {
            return None;
        }

        // Step down one level
        let old = self.level;
        self.level = match self.level {
            WarningLevel::Red => WarningLevel::Orange,
            WarningLevel::Orange => WarningLevel::Yellow,
            WarningLevel::Yellow => WarningLevel::Green,
            WarningLevel::Green => WarningLevel::Green,
        };

        if self.level != old {
            self.last_trigger_time = Some(now);
            self.record_transition(old, self.level, WarningTrigger::AllClear, now);
            info!("Warnstufe gesenkt: {} -> {}", old, self.level);
            Some(self.level)
        } else {
            None
        }
    }

    fn compute_target_level(&self) -> WarningLevel {
        self.active_triggers
            .iter()
            .map(|t| t.target_level())
            .max()
            .unwrap_or(WarningLevel::Green)
    }

    fn record_transition(
        &mut self,
        from: WarningLevel,
        to: WarningLevel,
        trigger: WarningTrigger,
        at: Instant,
    ) {
        if self.history.len() >= self.max_history {
            self.history.pop_front();
        }
        self.history.push_back(Transition {
            from,
            to,
            trigger,
            at,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_initial_state_is_green() {
        let sm = WarningStateMachine::new(120);
        assert_eq!(sm.current_level(), WarningLevel::Green);
    }

    #[test]
    fn test_aer_threshold_escalates_to_yellow() {
        let mut sm = WarningStateMachine::new(120);
        let result = sm.process_trigger(WarningTrigger::AerThreshold);
        assert_eq!(result, Some(WarningLevel::Yellow));
        assert_eq!(sm.current_level(), WarningLevel::Yellow);
    }

    #[test]
    fn test_aer_burst_escalates_to_orange() {
        let mut sm = WarningStateMachine::new(120);
        let result = sm.process_trigger(WarningTrigger::AerBurst);
        assert_eq!(result, Some(WarningLevel::Orange));
        assert_eq!(sm.current_level(), WarningLevel::Orange);
    }

    #[test]
    fn test_link_down_escalates_to_orange() {
        let mut sm = WarningStateMachine::new(120);
        let result = sm.process_trigger(WarningTrigger::LinkDown);
        assert_eq!(result, Some(WarningLevel::Orange));
    }

    #[test]
    fn test_no_duplicate_escalation() {
        let mut sm = WarningStateMachine::new(120);
        sm.process_trigger(WarningTrigger::AerBurst);
        // Same trigger again should not change level
        let result = sm.process_trigger(WarningTrigger::AerBurst);
        assert_eq!(result, None);
        assert_eq!(sm.current_level(), WarningLevel::Orange);
    }

    #[test]
    fn test_higher_trigger_escalates_further() {
        let mut sm = WarningStateMachine::new(120);
        sm.process_trigger(WarningTrigger::AerThreshold); // -> Yellow
        let result = sm.process_trigger(WarningTrigger::AerBurst); // -> Orange
        assert_eq!(result, Some(WarningLevel::Orange));
    }

    #[test]
    fn test_lower_trigger_does_not_deescalate() {
        let mut sm = WarningStateMachine::new(120);
        sm.process_trigger(WarningTrigger::AerBurst); // -> Orange
        let result = sm.process_trigger(WarningTrigger::AerThreshold); // Still Orange
        assert_eq!(result, None);
        assert_eq!(sm.current_level(), WarningLevel::Orange);
    }

    #[test]
    fn test_hysteresis_prevents_immediate_stepdown() {
        let mut sm = WarningStateMachine::new(120); // 120s cooldown
        sm.process_trigger(WarningTrigger::AerThreshold); // -> Yellow

        // AllClear immediately -> cooldown blocks stepdown
        let result = sm.process_trigger(WarningTrigger::AllClear);
        assert_eq!(result, None);
        assert_eq!(sm.current_level(), WarningLevel::Yellow);
    }

    #[test]
    fn test_stepdown_after_cooldown() {
        // Use 0 second cooldown for test
        let mut sm = WarningStateMachine::new(0);
        sm.process_trigger(WarningTrigger::AerThreshold); // -> Yellow

        // With 0 cooldown, AllClear should step down
        let result = sm.process_trigger(WarningTrigger::AllClear);
        assert_eq!(result, Some(WarningLevel::Green));
        assert_eq!(sm.current_level(), WarningLevel::Green);
    }

    #[test]
    fn test_stepdown_is_gradual_from_orange() {
        let mut sm = WarningStateMachine::new(0);
        sm.process_trigger(WarningTrigger::AerBurst); // -> Orange

        // First AllClear: Orange -> Yellow
        let result = sm.process_trigger(WarningTrigger::AllClear);
        assert_eq!(result, Some(WarningLevel::Yellow));

        // Second AllClear: Yellow -> Green (need to wait for cooldown set by first step)
        // With 0 cooldown but last_trigger_time set to now, we need try_step_down
        let result = sm.try_step_down();
        assert_eq!(result, Some(WarningLevel::Green));
    }

    #[test]
    fn test_try_step_down_with_active_triggers() {
        let mut sm = WarningStateMachine::new(0);
        sm.process_trigger(WarningTrigger::AerThreshold); // -> Yellow

        // try_step_down should not step down while trigger is active
        let result = sm.try_step_down();
        assert_eq!(result, None);
        assert_eq!(sm.current_level(), WarningLevel::Yellow);
    }

    #[test]
    fn test_clear_specific_trigger() {
        let mut sm = WarningStateMachine::new(0);
        sm.process_trigger(WarningTrigger::AerThreshold); // -> Yellow
        sm.process_trigger(WarningTrigger::AerBurst); // -> Orange

        // Clear burst, but threshold remains -> should stay Yellow (max of remaining)
        let result = sm.clear_trigger(&WarningTrigger::AerBurst);
        assert_eq!(result, Some(WarningLevel::Yellow));
        assert_eq!(sm.current_level(), WarningLevel::Yellow);
    }

    #[test]
    fn test_migration_actions_green() {
        let sm = WarningStateMachine::new(120);
        let actions = sm.migration_actions(&[
            ("worker1".to_string(), 1),
            ("worker2".to_string(), 5),
        ]);
        assert!(actions.is_empty());
    }

    #[test]
    fn test_migration_actions_yellow() {
        let mut sm = WarningStateMachine::new(120);
        sm.process_trigger(WarningTrigger::AerThreshold);

        let actions = sm.migration_actions(&[
            ("worker1".to_string(), 1),
            ("worker2".to_string(), 4),
            ("worker3".to_string(), 5),
        ]);

        // Should block new tasks
        assert!(actions.iter().any(|a| a.action == MigrationActionType::BlockNewTasks));
        // Should migrate prio 4 and 5
        let migrated: Vec<_> = actions
            .iter()
            .filter(|a| a.action == MigrationActionType::Migrate)
            .collect();
        assert_eq!(migrated.len(), 2);
        assert!(migrated.iter().any(|a| a.pipeline_name == "worker2"));
        assert!(migrated.iter().any(|a| a.pipeline_name == "worker3"));
    }

    #[test]
    fn test_migration_actions_orange() {
        let mut sm = WarningStateMachine::new(120);
        sm.process_trigger(WarningTrigger::AerBurst);

        let actions = sm.migration_actions(&[
            ("worker1".to_string(), 1),
            ("worker2".to_string(), 3),
        ]);

        // All should be migrated
        let migrated: Vec<_> = actions
            .iter()
            .filter(|a| a.action == MigrationActionType::Migrate)
            .collect();
        assert_eq!(migrated.len(), 2);
    }

    #[test]
    fn test_remigration_order() {
        let sm = WarningStateMachine::new(0); // Already Green

        let order = sm.remigration_order(&[
            ("worker_high".to_string(), 5),
            ("worker_low".to_string(), 1),
            ("worker_mid".to_string(), 3),
        ]);

        assert_eq!(order.len(), 3);
        // Sorted by priority ascending (lowest first)
        assert_eq!(order[0].pipeline_name, "worker_low");
        assert_eq!(order[1].pipeline_name, "worker_mid");
        assert_eq!(order[2].pipeline_name, "worker_high");
        assert!(order.iter().all(|a| a.action == MigrationActionType::ReMigrate));
    }

    #[test]
    fn test_remigration_not_available_when_not_green() {
        let mut sm = WarningStateMachine::new(120);
        sm.process_trigger(WarningTrigger::AerThreshold);

        let order = sm.remigration_order(&[("w".to_string(), 1)]);
        assert!(order.is_empty());
    }

    #[test]
    fn test_history_recorded() {
        let mut sm = WarningStateMachine::new(0);
        sm.process_trigger(WarningTrigger::AerThreshold);
        sm.process_trigger(WarningTrigger::AerBurst);

        assert_eq!(sm.history().len(), 2);
        assert_eq!(sm.history()[0].from, WarningLevel::Green);
        assert_eq!(sm.history()[0].to, WarningLevel::Yellow);
        assert_eq!(sm.history()[1].from, WarningLevel::Yellow);
        assert_eq!(sm.history()[1].to, WarningLevel::Orange);
    }

    #[test]
    fn test_multiple_triggers_same_level() {
        let mut sm = WarningStateMachine::new(0);
        sm.process_trigger(WarningTrigger::LinkSpeedDegradation); // -> Yellow
        let result = sm.process_trigger(WarningTrigger::AerThreshold); // Also Yellow, no change
        assert_eq!(result, None);
        assert_eq!(sm.current_level(), WarningLevel::Yellow);
        assert_eq!(sm.active_triggers().len(), 2);
    }

    #[test]
    fn test_cmplto_pattern_goes_orange() {
        let mut sm = WarningStateMachine::new(120);
        let result = sm.process_trigger(WarningTrigger::CmpltToPattern);
        assert_eq!(result, Some(WarningLevel::Orange));
    }

    #[test]
    fn test_nvidia_smi_timeout_goes_orange() {
        let mut sm = WarningStateMachine::new(120);
        let result = sm.process_trigger(WarningTrigger::NvidiaSmiTimeout);
        assert_eq!(result, Some(WarningLevel::Orange));
    }

    #[test]
    fn test_cuda_watchdog_goes_orange() {
        let mut sm = WarningStateMachine::new(120);
        let result = sm.process_trigger(WarningTrigger::CudaWatchdogTimeout);
        assert_eq!(result, Some(WarningLevel::Orange));
    }

    #[test]
    fn test_gpu_progress_error_goes_red() {
        let mut sm = WarningStateMachine::new(120);
        let result = sm.process_trigger(WarningTrigger::GpuProgressError);
        assert_eq!(result, Some(WarningLevel::Red));
        assert_eq!(sm.current_level(), WarningLevel::Red);
    }

    #[test]
    fn test_thermal_throttle_goes_yellow() {
        let mut sm = WarningStateMachine::new(120);
        let result = sm.process_trigger(WarningTrigger::ThermalThrottle);
        assert_eq!(result, Some(WarningLevel::Yellow));
    }

    #[test]
    fn test_thermal_critical_goes_orange() {
        let mut sm = WarningStateMachine::new(120);
        let result = sm.process_trigger(WarningTrigger::ThermalCritical);
        assert_eq!(result, Some(WarningLevel::Orange));
    }

    #[test]
    fn test_pstate_throttle_goes_yellow() {
        let mut sm = WarningStateMachine::new(120);
        let result = sm.process_trigger(WarningTrigger::PstateThrottle);
        assert_eq!(result, Some(WarningLevel::Yellow));
    }

    #[test]
    fn test_nvidia_smi_slow_goes_yellow() {
        let mut sm = WarningStateMachine::new(120);
        let result = sm.process_trigger(WarningTrigger::NvidiaSmiSlow);
        assert_eq!(result, Some(WarningLevel::Yellow));
    }

    #[test]
    fn test_health_score_low_goes_yellow() {
        let mut sm = WarningStateMachine::new(120);
        let result = sm.process_trigger(WarningTrigger::HealthScoreLow);
        assert_eq!(result, Some(WarningLevel::Yellow));
    }

    #[test]
    fn test_health_score_critical_goes_orange() {
        let mut sm = WarningStateMachine::new(120);
        let result = sm.process_trigger(WarningTrigger::HealthScoreCritical);
        assert_eq!(result, Some(WarningLevel::Orange));
    }
}
