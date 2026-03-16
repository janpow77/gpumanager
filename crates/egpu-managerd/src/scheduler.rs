use std::collections::{HashMap, VecDeque};

use egpu_manager_common::gpu::WarningLevel;
use serde::{Deserialize, Serialize};
use tracing::{debug, info, warn};

/// Which GPU a workload is assigned to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum GpuTarget {
    Egpu,
    Internal,
}

impl std::fmt::Display for GpuTarget {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GpuTarget::Egpu => write!(f, "eGPU"),
            GpuTarget::Internal => write!(f, "intern"),
        }
    }
}

/// Manual eGPU admission state set via API.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AdmissionState {
    /// Normal operation: eGPU accepts new tasks
    Open,
    /// Draining: no new tasks accepted, waiting for running tasks to finish
    Drain,
    /// Closed: all new eGPU tasks blocked immediately
    Closed,
}

impl std::fmt::Display for AdmissionState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AdmissionState::Open => write!(f, "open"),
            AdmissionState::Drain => write!(f, "drain"),
            AdmissionState::Closed => write!(f, "closed"),
        }
    }
}

/// A pipeline assignment in the scheduler.
#[derive(Debug, Clone)]
pub struct PipelineAssignment {
    pub name: String,
    pub priority: u32,
    pub vram_estimate_mb: u64,
    pub actual_vram_mb: u64,
    pub target: GpuTarget,
    pub preferred_target: GpuTarget,
}

/// A temporary VRAM reservation for an external GPU lease.
#[derive(Debug, Clone)]
pub struct LeaseReservation {
    pub target: GpuTarget,
    pub vram_mb: u64,
}

/// A request to schedule a pipeline.
#[derive(Debug, Clone)]
pub struct ScheduleRequest {
    pub name: String,
    pub priority: u32,
    pub vram_estimate_mb: u64,
    pub preferred_target: GpuTarget,
}

/// Result of a scheduling attempt.
#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(dead_code)]
pub enum ScheduleResult {
    /// Assigned to the requested GPU
    Assigned(GpuTarget),
    /// Preempted another pipeline to make room, assigned to the requested GPU
    PreemptedAndAssigned {
        target: GpuTarget,
        preempted: Vec<String>,
    },
    /// Queued because no capacity available
    Queued,
    /// Rejected: eGPU blocked by warning level
    BlockedByWarning,
}

/// GPU capacity information.
#[derive(Debug, Clone)]
pub struct GpuCapacity {
    pub total_vram_mb: u64,
    pub display_reserve_mb: u64,
}

impl GpuCapacity {
    pub fn available_vram_mb(&self) -> u64 {
        self.total_vram_mb.saturating_sub(self.display_reserve_mb)
    }
}

/// The VRAM scheduler.
///
/// Tracks which pipelines are on which GPU and manages scheduling
/// based on priority, VRAM capacity, and warning levels.
pub struct VramScheduler {
    egpu_capacity: GpuCapacity,
    internal_capacity: GpuCapacity,
    /// Currently assigned pipelines, keyed by name.
    assignments: HashMap<String, PipelineAssignment>,
    /// Temporary lease-based reservations for external applications.
    lease_reservations: HashMap<String, LeaseReservation>,
    /// Queue of pending requests that could not be assigned.
    queue: VecDeque<ScheduleRequest>,
    /// Current warning level (affects eGPU scheduling).
    warning_level: WarningLevel,
    /// Compute utilization threshold (percent). Above this, no new tasks.
    compute_threshold_percent: u32,
    /// Current compute utilization per GPU.
    compute_utilization: HashMap<GpuTarget, u32>,
    /// Whether eGPU is physically available.
    egpu_available: bool,
    /// Manual eGPU admission state (set via API).
    admission_state: AdmissionState,
}

impl VramScheduler {
    pub fn new(
        egpu_capacity: GpuCapacity,
        internal_capacity: GpuCapacity,
        compute_threshold_percent: u32,
    ) -> Self {
        Self {
            egpu_capacity,
            internal_capacity,
            assignments: HashMap::new(),
            lease_reservations: HashMap::new(),
            queue: VecDeque::new(),
            warning_level: WarningLevel::Green,
            compute_threshold_percent,
            compute_utilization: HashMap::new(),
            egpu_available: true,
            admission_state: AdmissionState::Open,
        }
    }

    #[allow(dead_code)]
    pub fn set_egpu_available(&mut self, available: bool) {
        self.egpu_available = available;
        if !available {
            info!("eGPU nicht verfügbar — alle Workloads auf interner GPU");
        }
    }

    pub fn set_warning_level(&mut self, level: WarningLevel) {
        self.warning_level = level;
    }

    pub fn set_compute_utilization(&mut self, gpu: GpuTarget, percent: u32) {
        self.compute_utilization.insert(gpu, percent);
    }

    /// Set the manual eGPU admission state.
    pub fn set_admission_state(&mut self, state: AdmissionState) {
        info!("eGPU-Admission geaendert: {}", state);
        self.admission_state = state;
    }

    /// Whether the physical eGPU is currently available.
    pub fn egpu_available(&self) -> bool {
        self.egpu_available
    }

    /// Whether a new workload of the given priority may be placed on the eGPU.
    pub fn egpu_allows_priority(&self, priority: u32) -> bool {
        self.egpu_available && !self.is_egpu_blocked_for_priority(priority)
    }

    /// Get the current eGPU admission state.
    pub fn admission_state(&self) -> AdmissionState {
        self.admission_state
    }

    /// Compute the effective admission state considering both the manual
    /// setting and the warning level.
    pub fn effective_admission_state(&self) -> &'static str {
        // Manuelle Sperre hat Vorrang
        if self.admission_state != AdmissionState::Open {
            return match self.admission_state {
                AdmissionState::Open => "open",
                AdmissionState::Drain => "drain",
                AdmissionState::Closed => "closed",
            };
        }
        // Warnstufen-basierte Sperre
        if self.warning_level >= WarningLevel::Yellow {
            "blocked"
        } else {
            "open"
        }
    }

    pub fn update_actual_vram(&mut self, name: &str, vram_mb: u64) {
        if let Some(assignment) = self.assignments.get_mut(name) {
            assignment.actual_vram_mb = vram_mb;
        }
    }

    /// Get all current assignments.
    pub fn assignments(&self) -> &HashMap<String, PipelineAssignment> {
        &self.assignments
    }

    /// Get mutable access to assignments (for priority changes etc.).
    pub fn assignments_mut(&mut self) -> &mut HashMap<String, PipelineAssignment> {
        &mut self.assignments
    }

    /// Add a temporary VRAM reservation for an externally managed GPU lease.
    pub fn reserve_lease(&mut self, lease_id: String, target: GpuTarget, vram_mb: u64) {
        self.lease_reservations
            .insert(lease_id, LeaseReservation { target, vram_mb });
    }

    /// Remove a temporary VRAM reservation for an external GPU lease.
    pub fn release_lease(&mut self, lease_id: &str) -> Option<LeaseReservation> {
        self.lease_reservations.remove(lease_id)
    }

    /// Total VRAM reserved by active leases on a GPU.
    pub fn reserved_vram(&self, target: GpuTarget) -> u64 {
        self.lease_reservations
            .values()
            .filter(|r| r.target == target)
            .map(|r| r.vram_mb)
            .sum()
    }

    /// Get pipelines assigned to a specific GPU, sorted by priority (highest first).
    pub fn pipelines_on_gpu(&self, target: GpuTarget) -> Vec<&PipelineAssignment> {
        let mut result: Vec<_> = self
            .assignments
            .values()
            .filter(|a| a.target == target)
            .collect();
        result.sort_by(|a, b| b.priority.cmp(&a.priority));
        result
    }

    /// Total VRAM used on a GPU.
    pub fn vram_used(&self, target: GpuTarget) -> u64 {
        let assigned_vram: u64 = self
            .assignments
            .values()
            .filter(|a| a.target == target)
            .map(|a| {
                if a.actual_vram_mb > 0 {
                    a.actual_vram_mb
                } else {
                    a.vram_estimate_mb
                }
            })
            .sum();

        assigned_vram + self.reserved_vram(target)
    }

    /// Available VRAM on a GPU.
    pub fn vram_available(&self, target: GpuTarget) -> u64 {
        let capacity = match target {
            GpuTarget::Egpu => &self.egpu_capacity,
            GpuTarget::Internal => &self.internal_capacity,
        };
        capacity
            .available_vram_mb()
            .saturating_sub(self.vram_used(target))
    }

    /// Schedule a pipeline for GPU assignment.
    pub fn schedule(&mut self, request: ScheduleRequest) -> ScheduleResult {
        // If already assigned, skip
        if self.assignments.contains_key(&request.name) {
            debug!("Pipeline {} ist bereits zugewiesen", request.name);
            return ScheduleResult::Assigned(self.assignments[&request.name].target);
        }

        let target = self.resolve_target(&request);

        match target {
            None => {
                // Try to preempt lower-priority pipelines
                if let Some(preempt_result) = self.try_preempt(&request) {
                    return preempt_result;
                }
                // No room even after preemption, queue it
                info!(
                    "Pipeline {} in Warteschlange (kein VRAM verfügbar)",
                    request.name
                );
                self.queue.push_back(request);
                ScheduleResult::Queued
            }
            Some(gpu) => {
                self.assign(&request, gpu);
                ScheduleResult::Assigned(gpu)
            }
        }
    }

    /// Remove a pipeline from assignments.
    #[allow(dead_code)]
    pub fn remove(&mut self, name: &str) -> Option<PipelineAssignment> {
        let removed = self.assignments.remove(name);
        if removed.is_some() {
            info!("Pipeline {} entfernt", name);
            self.try_dequeue();
        }
        removed
    }

    /// Migrate a pipeline to a different GPU.
    pub fn migrate(&mut self, name: &str, new_target: GpuTarget) -> bool {
        let Some(current) = self.assignments.get(name).cloned() else {
            return false;
        };

        if current.target == new_target {
            return true;
        }

        if new_target == GpuTarget::Egpu && self.is_egpu_blocked_for_priority(current.priority) {
            warn!(
                "Migration von {} nach {} abgelehnt: eGPU blockiert",
                name, new_target
            );
            return false;
        }

        let current_vram = if current.actual_vram_mb > 0 {
            current.actual_vram_mb
        } else {
            current.vram_estimate_mb
        };

        let target_capacity = match new_target {
            GpuTarget::Egpu => &self.egpu_capacity,
            GpuTarget::Internal => &self.internal_capacity,
        };

        let target_used_without_current: u64 = self
            .assignments
            .values()
            .filter(|a| a.target == new_target && a.name != name)
            .map(|a| {
                if a.actual_vram_mb > 0 {
                    a.actual_vram_mb
                } else {
                    a.vram_estimate_mb
                }
            })
            .sum::<u64>()
            + self.reserved_vram(new_target);

        let available = target_capacity
            .available_vram_mb()
            .saturating_sub(target_used_without_current);

        if available < current_vram {
            warn!(
                "Migration von {} nach {} abgelehnt: nicht genug VRAM ({} MB frei, {} MB benoetigt)",
                name, new_target, available, current_vram
            );
            return false;
        }

        if let Some(assignment) = self.assignments.get_mut(name) {
            let old = assignment.target;
            assignment.target = new_target;
            info!("Pipeline {} migriert: {} -> {}", name, old, new_target);
            true
        } else {
            false
        }
    }

    /// Get the current queue.
    pub fn queue(&self) -> &VecDeque<ScheduleRequest> {
        &self.queue
    }

    /// Update a pipeline's current workload type and VRAM from a webhook.
    #[allow(dead_code)]
    pub fn update_workload(
        &mut self,
        pipeline: &str,
        workload_type: &str,
        vram_mb: u64,
        gpu_active: bool,
    ) -> bool {
        if let Some(assignment) = self.assignments.get_mut(pipeline) {
            assignment.actual_vram_mb = vram_mb;
            debug!(
                "Workload-Update: {} — Typ: {}, VRAM: {} MB, GPU aktiv: {}",
                pipeline, workload_type, vram_mb, gpu_active
            );
            true
        } else {
            warn!("Workload-Update für unbekannte Pipeline: {}", pipeline);
            false
        }
    }

    /// Prueft ob eGPU fuer eine gegebene Prioritaet gesperrt ist.
    /// Beruecksichtigt manuelle Admission-Sperre, Warnstufen und
    /// proaktives Throttling nach Prioritaet.
    fn is_egpu_blocked_for_priority(&self, priority: u32) -> bool {
        // Manuelle Sperre: Closed oder Drain blockiert alle neuen Tasks
        if self.admission_state == AdmissionState::Closed
            || self.admission_state == AdmissionState::Drain
        {
            return true;
        }

        // Proaktives Throttling basierend auf Warnstufe:
        // - YELLOW: Blockiere niedrige Prioritaet (>= 4)
        // - ORANGE: Blockiere alle (Migration aller nicht-essentiellen Tasks)
        // - RED: Blockiere ALLE neuen eGPU-Tasks
        match self.warning_level {
            WarningLevel::Green => false,
            WarningLevel::Yellow => {
                if priority >= 4 {
                    debug!("eGPU blockiert: Warnstufe YELLOW, Prio {} >= 4", priority);
                    true
                } else {
                    false
                }
            }
            WarningLevel::Orange | WarningLevel::Red => {
                debug!(
                    "eGPU blockiert: Warnstufe {} blockiert alle neuen Tasks",
                    self.warning_level
                );
                true
            }
        }
    }

    fn resolve_target(&self, request: &ScheduleRequest) -> Option<GpuTarget> {
        // If eGPU not available, always use internal
        if !self.egpu_available {
            return self.check_capacity(GpuTarget::Internal, request.vram_estimate_mb);
        }

        // Pruefe eGPU-Blockierung (manuell + warnstufen-basiert + proaktives Throttling)
        if request.preferred_target == GpuTarget::Egpu
            && self.is_egpu_blocked_for_priority(request.priority)
        {
            debug!(
                "eGPU blockiert fuer {} (Admission: {}, Warnstufe: {}, Prio: {})",
                request.name, self.admission_state, self.warning_level, request.priority
            );
            // Fall back to internal if possible
            return self.check_capacity(GpuTarget::Internal, request.vram_estimate_mb);
        }

        // Check compute utilization
        let util = self
            .compute_utilization
            .get(&request.preferred_target)
            .copied()
            .unwrap_or(0);
        if util > self.compute_threshold_percent {
            debug!(
                "GPU {} Auslastung zu hoch ({}% > {}%)",
                request.preferred_target, util, self.compute_threshold_percent
            );
            // Try the other GPU
            let alt = match request.preferred_target {
                GpuTarget::Egpu => GpuTarget::Internal,
                GpuTarget::Internal => GpuTarget::Egpu,
            };
            return self.check_capacity(alt, request.vram_estimate_mb);
        }

        // Try preferred target first
        if let Some(gpu) = self.check_capacity(request.preferred_target, request.vram_estimate_mb) {
            return Some(gpu);
        }

        // Try fallback
        let alt = match request.preferred_target {
            GpuTarget::Egpu => GpuTarget::Internal,
            GpuTarget::Internal => GpuTarget::Egpu,
        };
        self.check_capacity(alt, request.vram_estimate_mb)
    }

    fn check_capacity(&self, target: GpuTarget, needed_mb: u64) -> Option<GpuTarget> {
        if target == GpuTarget::Egpu && !self.egpu_available {
            return None;
        }
        // Pruefe eGPU-Blockierung bei ORANGE/RED (alle Tasks blockiert)
        if target == GpuTarget::Egpu
            && (self.warning_level >= WarningLevel::Orange
                || self.admission_state == AdmissionState::Closed
                || self.admission_state == AdmissionState::Drain)
        {
            return None;
        }

        if self.vram_available(target) >= needed_mb {
            Some(target)
        } else {
            None
        }
    }

    fn assign(&mut self, request: &ScheduleRequest, target: GpuTarget) {
        info!(
            "Pipeline {} zugewiesen: {} (Prio {}, ~{} MB VRAM)",
            request.name, target, request.priority, request.vram_estimate_mb
        );
        self.assignments.insert(
            request.name.clone(),
            PipelineAssignment {
                name: request.name.clone(),
                priority: request.priority,
                vram_estimate_mb: request.vram_estimate_mb,
                actual_vram_mb: 0,
                target,
                preferred_target: request.preferred_target,
            },
        );
    }

    fn try_preempt(&mut self, request: &ScheduleRequest) -> Option<ScheduleResult> {
        let target = request.preferred_target;
        if target == GpuTarget::Egpu
            && (!self.egpu_available || self.is_egpu_blocked_for_priority(request.priority))
        {
            return None;
        }

        // Find lower-priority pipelines on the target GPU that can be preempted
        let mut preemptable: Vec<(String, u32, u64)> = self
            .assignments
            .values()
            .filter(|a| a.target == target && a.priority > request.priority)
            .map(|a| {
                let vram = if a.actual_vram_mb > 0 {
                    a.actual_vram_mb
                } else {
                    a.vram_estimate_mb
                };
                (a.name.clone(), a.priority, vram)
            })
            .collect();

        // Sort by priority descending (preempt lowest priority first)
        preemptable.sort_by(|a, b| b.1.cmp(&a.1));

        let mut freed: u64 = self.vram_available(target);
        let mut preempted = Vec::new();

        for (name, _, vram) in &preemptable {
            if freed >= request.vram_estimate_mb {
                break;
            }
            freed += vram;
            preempted.push(name.clone());
        }

        if freed >= request.vram_estimate_mb && !preempted.is_empty() {
            // Actually remove the preempted pipelines
            for name in &preempted {
                if let Some(removed) = self.assignments.remove(name) {
                    warn!(
                        "Pipeline {} verdrängt (Prio {} < {})",
                        name, removed.priority, request.priority
                    );
                    // Re-queue the preempted pipeline
                    self.queue.push_back(ScheduleRequest {
                        name: removed.name,
                        priority: removed.priority,
                        vram_estimate_mb: removed.vram_estimate_mb,
                        preferred_target: removed.preferred_target,
                    });
                }
            }

            self.assign(request, target);
            Some(ScheduleResult::PreemptedAndAssigned { target, preempted })
        } else {
            None
        }
    }

    /// Return eGPU pipelines sorted by priority descending (lowest priority first to shed).
    /// Used by pressure reduction to find candidates for shedding load.
    pub fn pressure_reduction_candidates(&self) -> Vec<&PipelineAssignment> {
        let mut candidates: Vec<_> = self
            .assignments
            .values()
            .filter(|a| a.target == GpuTarget::Egpu)
            .collect();
        // Sort by priority descending (highest number = lowest importance = shed first)
        candidates.sort_by(|a, b| b.priority.cmp(&a.priority));
        candidates
    }

    #[allow(dead_code)]
    fn try_dequeue(&mut self) {
        // Try to assign queued requests
        let mut retry = Vec::new();
        while let Some(request) = self.queue.pop_front() {
            if let Some(target) = self.resolve_target(&request) {
                self.assign(&request, target);
            } else {
                retry.push(request);
            }
        }
        self.queue = VecDeque::from(retry);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_scheduler() -> VramScheduler {
        VramScheduler::new(
            GpuCapacity {
                total_vram_mb: 16000,
                display_reserve_mb: 0,
            },
            GpuCapacity {
                total_vram_mb: 8000,
                display_reserve_mb: 512,
            },
            90,
        )
    }

    #[test]
    fn test_simple_assignment() {
        let mut sched = make_scheduler();
        let result = sched.schedule(ScheduleRequest {
            name: "worker1".to_string(),
            priority: 1,
            vram_estimate_mb: 4000,
            preferred_target: GpuTarget::Egpu,
        });
        assert_eq!(result, ScheduleResult::Assigned(GpuTarget::Egpu));
        assert_eq!(sched.vram_used(GpuTarget::Egpu), 4000);
        assert_eq!(sched.vram_available(GpuTarget::Egpu), 12000);
    }

    #[test]
    fn test_fallback_to_internal_when_egpu_full() {
        let mut sched = make_scheduler();
        // Fill eGPU
        sched.schedule(ScheduleRequest {
            name: "big_worker".to_string(),
            priority: 1,
            vram_estimate_mb: 15000,
            preferred_target: GpuTarget::Egpu,
        });

        // Next request should fall back to internal
        let result = sched.schedule(ScheduleRequest {
            name: "small_worker".to_string(),
            priority: 2,
            vram_estimate_mb: 2000,
            preferred_target: GpuTarget::Egpu,
        });
        assert_eq!(result, ScheduleResult::Assigned(GpuTarget::Internal));
    }

    #[test]
    fn test_queued_when_no_capacity() {
        let mut sched = make_scheduler();
        // Fill both GPUs
        sched.schedule(ScheduleRequest {
            name: "big1".to_string(),
            priority: 1,
            vram_estimate_mb: 16000,
            preferred_target: GpuTarget::Egpu,
        });
        sched.schedule(ScheduleRequest {
            name: "big2".to_string(),
            priority: 1,
            vram_estimate_mb: 7488,
            preferred_target: GpuTarget::Internal,
        });

        let result = sched.schedule(ScheduleRequest {
            name: "extra".to_string(),
            priority: 3,
            vram_estimate_mb: 1000,
            preferred_target: GpuTarget::Egpu,
        });
        assert_eq!(result, ScheduleResult::Queued);
        assert_eq!(sched.queue().len(), 1);
    }

    #[test]
    fn test_preemption() {
        let mut sched = make_scheduler();
        // Low priority takes all eGPU VRAM
        sched.schedule(ScheduleRequest {
            name: "low_prio".to_string(),
            priority: 5,
            vram_estimate_mb: 14000,
            preferred_target: GpuTarget::Egpu,
        });

        // High priority should preempt
        let result = sched.schedule(ScheduleRequest {
            name: "high_prio".to_string(),
            priority: 1,
            vram_estimate_mb: 14000,
            preferred_target: GpuTarget::Egpu,
        });

        match result {
            ScheduleResult::PreemptedAndAssigned { target, preempted } => {
                assert_eq!(target, GpuTarget::Egpu);
                assert_eq!(preempted, vec!["low_prio"]);
            }
            other => panic!("Expected PreemptedAndAssigned, got {other:?}"),
        }

        // low_prio should be in queue
        assert_eq!(sched.queue().len(), 1);
        assert_eq!(sched.queue()[0].name, "low_prio");
    }

    #[test]
    fn test_no_preemption_of_higher_priority() {
        let mut sched = make_scheduler();
        sched.schedule(ScheduleRequest {
            name: "high_prio".to_string(),
            priority: 1,
            vram_estimate_mb: 14000,
            preferred_target: GpuTarget::Egpu,
        });

        // Lower priority cannot preempt higher
        let result = sched.schedule(ScheduleRequest {
            name: "low_prio".to_string(),
            priority: 5,
            vram_estimate_mb: 14000,
            preferred_target: GpuTarget::Egpu,
        });

        // Should try internal GPU since it can't preempt
        // Internal has 7488 available, 14000 needed -> queued
        assert_eq!(result, ScheduleResult::Queued);
    }

    #[test]
    fn test_warning_yellow_blocks_low_priority_egpu() {
        let mut sched = make_scheduler();
        sched.set_warning_level(WarningLevel::Yellow);

        // Niedrige Prioritaet (>= 4) wird bei YELLOW blockiert
        let result = sched.schedule(ScheduleRequest {
            name: "low_prio_worker".to_string(),
            priority: 4,
            vram_estimate_mb: 4000,
            preferred_target: GpuTarget::Egpu,
        });
        assert_eq!(result, ScheduleResult::Assigned(GpuTarget::Internal));

        // Hohe Prioritaet (< 4) darf bei YELLOW noch auf eGPU
        let result = sched.schedule(ScheduleRequest {
            name: "high_prio_worker".to_string(),
            priority: 1,
            vram_estimate_mb: 4000,
            preferred_target: GpuTarget::Egpu,
        });
        assert_eq!(result, ScheduleResult::Assigned(GpuTarget::Egpu));
    }

    #[test]
    fn test_warning_orange_blocks_all_egpu() {
        let mut sched = make_scheduler();
        sched.set_warning_level(WarningLevel::Orange);

        // Bei ORANGE werden ALLE neuen eGPU-Tasks blockiert
        let result = sched.schedule(ScheduleRequest {
            name: "worker".to_string(),
            priority: 1,
            vram_estimate_mb: 4000,
            preferred_target: GpuTarget::Egpu,
        });
        assert_eq!(result, ScheduleResult::Assigned(GpuTarget::Internal));
    }

    #[test]
    fn test_admission_closed_blocks_egpu() {
        let mut sched = make_scheduler();
        sched.set_admission_state(AdmissionState::Closed);

        let result = sched.schedule(ScheduleRequest {
            name: "worker".to_string(),
            priority: 1,
            vram_estimate_mb: 4000,
            preferred_target: GpuTarget::Egpu,
        });
        assert_eq!(result, ScheduleResult::Assigned(GpuTarget::Internal));
    }

    #[test]
    fn test_admission_drain_blocks_egpu() {
        let mut sched = make_scheduler();
        sched.set_admission_state(AdmissionState::Drain);

        let result = sched.schedule(ScheduleRequest {
            name: "worker".to_string(),
            priority: 1,
            vram_estimate_mb: 4000,
            preferred_target: GpuTarget::Egpu,
        });
        assert_eq!(result, ScheduleResult::Assigned(GpuTarget::Internal));
    }

    #[test]
    fn test_admission_open_allows_egpu() {
        let mut sched = make_scheduler();
        sched.set_admission_state(AdmissionState::Open);

        let result = sched.schedule(ScheduleRequest {
            name: "worker".to_string(),
            priority: 1,
            vram_estimate_mb: 4000,
            preferred_target: GpuTarget::Egpu,
        });
        assert_eq!(result, ScheduleResult::Assigned(GpuTarget::Egpu));
    }

    #[test]
    fn test_effective_admission_state() {
        let mut sched = make_scheduler();

        // Default: open
        assert_eq!(sched.effective_admission_state(), "open");

        // Manuell geschlossen
        sched.set_admission_state(AdmissionState::Closed);
        assert_eq!(sched.effective_admission_state(), "closed");

        // Drain
        sched.set_admission_state(AdmissionState::Drain);
        assert_eq!(sched.effective_admission_state(), "drain");

        // Zurueck auf open, aber Warnstufe Yellow -> blocked
        sched.set_admission_state(AdmissionState::Open);
        sched.set_warning_level(WarningLevel::Yellow);
        assert_eq!(sched.effective_admission_state(), "blocked");

        // Green -> open
        sched.set_warning_level(WarningLevel::Green);
        assert_eq!(sched.effective_admission_state(), "open");
    }

    #[test]
    fn test_egpu_unavailable_uses_internal() {
        let mut sched = make_scheduler();
        sched.set_egpu_available(false);

        let result = sched.schedule(ScheduleRequest {
            name: "worker".to_string(),
            priority: 1,
            vram_estimate_mb: 4000,
            preferred_target: GpuTarget::Egpu,
        });

        assert_eq!(result, ScheduleResult::Assigned(GpuTarget::Internal));
    }

    #[test]
    fn test_dequeue_on_removal() {
        let mut sched = make_scheduler();

        // Fill eGPU
        sched.schedule(ScheduleRequest {
            name: "first".to_string(),
            priority: 1,
            vram_estimate_mb: 14000,
            preferred_target: GpuTarget::Egpu,
        });

        // Queue a second one (internal can hold it but prefers eGPU)
        // Fill internal too so it really queues
        sched.schedule(ScheduleRequest {
            name: "internal_fill".to_string(),
            priority: 1,
            vram_estimate_mb: 7000,
            preferred_target: GpuTarget::Internal,
        });

        sched.schedule(ScheduleRequest {
            name: "waiting".to_string(),
            priority: 2,
            vram_estimate_mb: 4000,
            preferred_target: GpuTarget::Egpu,
        });
        assert_eq!(sched.queue().len(), 1);

        // Remove first, freeing eGPU space — but warning level blocks eGPU
        // so let's keep it green
        sched.remove("first");

        // The queued item should have been auto-assigned
        assert_eq!(sched.queue().len(), 0);
        assert!(sched.assignments().contains_key("waiting"));
    }

    #[test]
    fn test_migrate() {
        let mut sched = make_scheduler();
        sched.schedule(ScheduleRequest {
            name: "worker".to_string(),
            priority: 1,
            vram_estimate_mb: 4000,
            preferred_target: GpuTarget::Egpu,
        });

        assert!(sched.migrate("worker", GpuTarget::Internal));
        assert_eq!(sched.assignments()["worker"].target, GpuTarget::Internal);
    }

    #[test]
    fn test_display_vram_reserve() {
        let sched = make_scheduler();
        // Internal: 8000 total - 512 reserve = 7488 available
        assert_eq!(sched.vram_available(GpuTarget::Internal), 7488);
    }

    #[test]
    fn test_compute_utilization_blocks() {
        let mut sched = make_scheduler();
        sched.set_compute_utilization(GpuTarget::Egpu, 95);

        let result = sched.schedule(ScheduleRequest {
            name: "worker".to_string(),
            priority: 1,
            vram_estimate_mb: 4000,
            preferred_target: GpuTarget::Egpu,
        });

        // Should fall back to internal due to high compute utilization
        assert_eq!(result, ScheduleResult::Assigned(GpuTarget::Internal));
    }

    #[test]
    fn test_pipelines_on_gpu() {
        let mut sched = make_scheduler();
        sched.schedule(ScheduleRequest {
            name: "w1".to_string(),
            priority: 3,
            vram_estimate_mb: 2000,
            preferred_target: GpuTarget::Egpu,
        });
        sched.schedule(ScheduleRequest {
            name: "w2".to_string(),
            priority: 1,
            vram_estimate_mb: 2000,
            preferred_target: GpuTarget::Egpu,
        });
        sched.schedule(ScheduleRequest {
            name: "w3".to_string(),
            priority: 5,
            vram_estimate_mb: 2000,
            preferred_target: GpuTarget::Internal,
        });

        let on_egpu = sched.pipelines_on_gpu(GpuTarget::Egpu);
        assert_eq!(on_egpu.len(), 2);
        // Sorted by priority descending (highest priority number first)
        assert_eq!(on_egpu[0].priority, 3);
        assert_eq!(on_egpu[1].priority, 1);

        let on_internal = sched.pipelines_on_gpu(GpuTarget::Internal);
        assert_eq!(on_internal.len(), 1);
    }

    #[test]
    fn test_duplicate_schedule_noop() {
        let mut sched = make_scheduler();
        sched.schedule(ScheduleRequest {
            name: "worker".to_string(),
            priority: 1,
            vram_estimate_mb: 4000,
            preferred_target: GpuTarget::Egpu,
        });

        // Scheduling again should just return existing assignment
        let result = sched.schedule(ScheduleRequest {
            name: "worker".to_string(),
            priority: 1,
            vram_estimate_mb: 4000,
            preferred_target: GpuTarget::Egpu,
        });
        assert_eq!(result, ScheduleResult::Assigned(GpuTarget::Egpu));
    }

    #[test]
    fn test_update_actual_vram() {
        let mut sched = make_scheduler();
        sched.schedule(ScheduleRequest {
            name: "worker".to_string(),
            priority: 1,
            vram_estimate_mb: 4000,
            preferred_target: GpuTarget::Egpu,
        });

        sched.update_actual_vram("worker", 3500);
        assert_eq!(sched.assignments()["worker"].actual_vram_mb, 3500);
        // VRAM used should now reflect actual
        assert_eq!(sched.vram_used(GpuTarget::Egpu), 3500);
    }
}
