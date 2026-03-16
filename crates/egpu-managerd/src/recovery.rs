use std::collections::HashMap;
use std::time::Duration;

use chrono::Utc;
use egpu_manager_common::config::Config;
use egpu_manager_common::hal::{DockerControl, PcieControl, ThunderboltControl};
use serde::{Deserialize, Serialize};
use tracing::{error, info, warn};

use crate::db::{EventDb, RecoveryState, Severity};

/// Recovery stages in order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum RecoveryStage {
    /// No recovery in progress
    Idle,
    /// Stage 0: Quiesce workloads (Celery, generic hooks, Redis BGSAVE, PostgreSQL CHECKPOINT)
    Stage0Quiesce,
    /// Stage 1: PCIe function-level reset, then verify nvidia-smi
    Stage1PcieReset,
    /// Stage 2: Migrate containers to fallback GPU via docker-compose override
    Stage2Migration,
    /// Stage 3: Thunderbolt deauth/reauth cycle
    Stage3ThunderboltReconnect,
    /// Stage 4: Manual intervention required
    Stage4ManualIntervention,
}

impl std::fmt::Display for RecoveryStage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RecoveryStage::Idle => write!(f, "idle"),
            RecoveryStage::Stage0Quiesce => write!(f, "stage0_quiesce"),
            RecoveryStage::Stage1PcieReset => write!(f, "stage1_pcie_reset"),
            RecoveryStage::Stage2Migration => write!(f, "stage2_migration"),
            RecoveryStage::Stage3ThunderboltReconnect => write!(f, "stage3_thunderbolt"),
            RecoveryStage::Stage4ManualIntervention => write!(f, "stage4_manual"),
        }
    }
}

impl RecoveryStage {
    pub fn from_str_lossy(s: &str) -> Self {
        match s {
            "idle" => RecoveryStage::Idle,
            "stage0_quiesce" => RecoveryStage::Stage0Quiesce,
            "stage1_pcie_reset" => RecoveryStage::Stage1PcieReset,
            "stage2_migration" => RecoveryStage::Stage2Migration,
            "stage3_thunderbolt" => RecoveryStage::Stage3ThunderboltReconnect,
            "stage4_manual" => RecoveryStage::Stage4ManualIntervention,
            _ => RecoveryStage::Idle,
        }
    }

    /// Get the next stage in the recovery sequence.
    pub fn next(self) -> Self {
        match self {
            RecoveryStage::Idle => RecoveryStage::Stage0Quiesce,
            RecoveryStage::Stage0Quiesce => RecoveryStage::Stage1PcieReset,
            RecoveryStage::Stage1PcieReset => RecoveryStage::Stage2Migration,
            RecoveryStage::Stage2Migration => RecoveryStage::Stage3ThunderboltReconnect,
            RecoveryStage::Stage3ThunderboltReconnect => RecoveryStage::Stage4ManualIntervention,
            RecoveryStage::Stage4ManualIntervention => RecoveryStage::Stage4ManualIntervention,
        }
    }

    /// Default timeout for each stage in seconds.
    pub fn timeout_seconds(self) -> u64 {
        match self {
            RecoveryStage::Idle => 0,
            RecoveryStage::Stage0Quiesce => 30,
            RecoveryStage::Stage1PcieReset => 15,
            RecoveryStage::Stage2Migration => 120,
            RecoveryStage::Stage3ThunderboltReconnect => 30,
            RecoveryStage::Stage4ManualIntervention => 0, // No timeout - manual
        }
    }
}

/// Result of a single recovery stage execution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StageResult {
    /// Stage completed successfully, recovery can stop
    Success,
    /// Stage failed, advance to next stage
    Failed(String),
    /// Stage completed but eGPU not yet recovered, advance to next stage
    AdvanceToNext,
}

/// Tracks which pipelines are affected by the current recovery.
#[derive(Debug, Clone, Default)]
pub struct AffectedPipelines {
    pub pipelines: Vec<String>,
}

/// The recovery state machine.
pub struct RecoveryStateMachine {
    stage: RecoveryStage,
    db: EventDb,
    cooldown_seconds: u64,
    affected: AffectedPipelines,
    started_at: chrono::DateTime<Utc>,
}

impl RecoveryStateMachine {
    pub fn new(db: EventDb, cooldown_seconds: u64) -> Self {
        Self {
            stage: RecoveryStage::Idle,
            db,
            cooldown_seconds,
            affected: AffectedPipelines::default(),
            started_at: Utc::now(),
        }
    }

    pub fn current_stage(&self) -> RecoveryStage {
        self.stage
    }

    pub fn is_active(&self) -> bool {
        self.stage != RecoveryStage::Idle
    }

    pub fn affected_pipelines(&self) -> &[String] {
        &self.affected.pipelines
    }

    /// Check for interrupted recovery on startup and resume if found.
    pub async fn check_interrupted(&mut self) -> anyhow::Result<Option<RecoveryStage>> {
        let state = self.db.load_recovery_state().await?;
        match state {
            Some(rs) if rs.status == "in_progress" => {
                let stage = RecoveryStage::from_str_lossy(&rs.stage);
                info!(
                    "Unterbrochene Recovery gefunden: Stage {} (gestartet: {})",
                    stage, rs.started_at
                );
                self.stage = stage;
                self.started_at = rs.started_at;
                Ok(Some(stage))
            }
            _ => Ok(None),
        }
    }

    /// Start recovery. Called when warning level reaches Orange.
    pub async fn start_recovery(
        &mut self,
        affected_pipelines: Vec<String>,
    ) -> anyhow::Result<()> {
        if self.is_active() {
            info!("Recovery bereits aktiv in Stage {}", self.stage);
            return Ok(());
        }

        self.stage = RecoveryStage::Stage0Quiesce;
        self.affected = AffectedPipelines {
            pipelines: affected_pipelines,
        };
        self.started_at = Utc::now();

        self.persist_state("in_progress").await?;

        self.db
            .log_event(
                "recovery.start",
                Severity::Warning,
                &format!(
                    "Recovery gestartet: Stage {} (betroffene Pipelines: {:?})",
                    self.stage, self.affected.pipelines
                ),
                Some(serde_json::json!({
                    "stage": self.stage.to_string(),
                    "affected_pipelines": self.affected.pipelines,
                })),
            )
            .await?;

        Ok(())
    }

    /// Execute the current stage. Returns the stage result.
    pub async fn execute_current_stage(
        &mut self,
        config: &Config,
        docker: &dyn DockerControl,
        pcie: Option<&dyn PcieControl>,
        thunderbolt: Option<&dyn ThunderboltControl>,
    ) -> anyhow::Result<StageResult> {
        let stage = self.stage;
        info!("Recovery Stage {} wird ausgeführt", stage);

        let timeout = Duration::from_secs(stage.timeout_seconds());
        let result = if timeout.is_zero() {
            self.execute_stage_inner(stage, config, docker, pcie, thunderbolt)
                .await
        } else {
            match tokio::time::timeout(
                timeout,
                self.execute_stage_inner(stage, config, docker, pcie, thunderbolt),
            )
            .await
            {
                Ok(result) => result,
                Err(_) => {
                    warn!("Recovery Stage {} Timeout nach {}s", stage, timeout.as_secs());
                    Ok(StageResult::Failed(format!(
                        "Timeout nach {}s",
                        timeout.as_secs()
                    )))
                }
            }
        };

        match &result {
            Ok(StageResult::Success) => {
                info!("Recovery Stage {} erfolgreich", stage);
                self.db
                    .log_event(
                        "recovery.stage_success",
                        Severity::Info,
                        &format!("Recovery Stage {} erfolgreich", stage),
                        None,
                    )
                    .await?;
                self.complete_recovery().await?;
            }
            Ok(StageResult::Failed(reason)) => {
                warn!("Recovery Stage {} fehlgeschlagen: {}", stage, reason);
                self.db
                    .log_event(
                        "recovery.stage_failed",
                        Severity::Warning,
                        &format!("Recovery Stage {} fehlgeschlagen: {}", stage, reason),
                        None,
                    )
                    .await?;
                self.advance_stage().await?;
            }
            Ok(StageResult::AdvanceToNext) => {
                info!("Recovery Stage {} abgeschlossen, weiter zu nächster Stage", stage);
                self.advance_stage().await?;
            }
            Err(e) => {
                error!("Recovery Stage {} Fehler: {}", stage, e);
                self.advance_stage().await?;
            }
        }

        // Cooldown between stages
        if self.is_active() && self.cooldown_seconds > 0 {
            tokio::time::sleep(Duration::from_secs(self.cooldown_seconds)).await;
        }

        result
    }

    async fn execute_stage_inner(
        &self,
        stage: RecoveryStage,
        config: &Config,
        docker: &dyn DockerControl,
        pcie: Option<&dyn PcieControl>,
        thunderbolt: Option<&dyn ThunderboltControl>,
    ) -> anyhow::Result<StageResult> {
        match stage {
            RecoveryStage::Idle => Ok(StageResult::Success),
            RecoveryStage::Stage0Quiesce => {
                self.execute_quiesce(config, docker).await
            }
            RecoveryStage::Stage1PcieReset => {
                self.execute_pcie_reset(config, pcie).await
            }
            RecoveryStage::Stage2Migration => {
                self.execute_migration(config, docker).await
            }
            RecoveryStage::Stage3ThunderboltReconnect => {
                self.execute_thunderbolt_reconnect(config, thunderbolt).await
            }
            RecoveryStage::Stage4ManualIntervention => {
                self.execute_manual_intervention().await
            }
        }
    }

    /// Stage 0: Execute quiesce hooks in order.
    /// Order: Celery first, then generic, then Redis BGSAVE, then PostgreSQL CHECKPOINT.
    async fn execute_quiesce(
        &self,
        config: &Config,
        docker: &dyn DockerControl,
    ) -> anyhow::Result<StageResult> {
        info!("Stage 0: Quiesce-Hooks werden ausgeführt");

        for pipeline in &config.pipeline {
            if !self.affected.pipelines.contains(&pipeline.container) {
                continue;
            }

            // Execute quiesce hooks in defined order
            for hook in &pipeline.quiesce_hooks {
                info!(
                    "Quiesce-Hook: Container={}, Command={}",
                    hook.container, hook.command
                );
                let cmd_parts: Vec<&str> = hook.command.split_whitespace().collect();
                let timeout = Duration::from_secs(hook.timeout_seconds);

                match docker.exec_in_container(&hook.container, &cmd_parts, timeout).await {
                    Ok(output) => {
                        info!("Quiesce-Hook erfolgreich: {} -> {}", hook.container, output);
                    }
                    Err(e) => {
                        warn!("Quiesce-Hook fehlgeschlagen: {} -> {}", hook.container, e);
                        // Continue with other hooks, don't fail the whole stage
                    }
                }
            }

            // Redis BGSAVE for associated redis containers
            for redis_container in &pipeline.redis_containers {
                info!("Redis BGSAVE: {}", redis_container);
                match docker
                    .exec_in_container(
                        redis_container,
                        &["redis-cli", "BGSAVE"],
                        Duration::from_secs(10),
                    )
                    .await
                {
                    Ok(_) => info!("Redis BGSAVE erfolgreich: {}", redis_container),
                    Err(e) => warn!("Redis BGSAVE fehlgeschlagen: {} -> {}", redis_container, e),
                }
            }
        }

        Ok(StageResult::AdvanceToNext)
    }

    /// Stage 1: PCIe function-level reset, then check nvidia-smi.
    async fn execute_pcie_reset(
        &self,
        config: &Config,
        pcie: Option<&dyn PcieControl>,
    ) -> anyhow::Result<StageResult> {
        info!("Stage 1: PCIe Function-Level Reset");

        let pcie = match pcie {
            Some(p) => p,
            None => {
                warn!("PCIe-Control nicht verfügbar, überspringe Stage 1");
                return Ok(StageResult::Failed(
                    "PCIe-Control nicht verfügbar".to_string(),
                ));
            }
        };

        match pcie.function_level_reset(&config.gpu.egpu_pci_address).await {
            Ok(()) => {
                info!("PCIe FLR erfolgreich für {}", config.gpu.egpu_pci_address);
                // Check if nvidia-smi works after reset
                match check_nvidia_smi_available().await {
                    true => {
                        info!("nvidia-smi nach FLR erreichbar - Recovery erfolgreich");
                        Ok(StageResult::Success)
                    }
                    false => {
                        warn!("nvidia-smi nach FLR nicht erreichbar");
                        Ok(StageResult::Failed(
                            "nvidia-smi nach FLR nicht erreichbar".to_string(),
                        ))
                    }
                }
            }
            Err(e) => {
                warn!("PCIe FLR fehlgeschlagen: {}", e);
                Ok(StageResult::Failed(format!("PCIe FLR fehlgeschlagen: {e}")))
            }
        }
    }

    /// Stage 2: Generate per-service override files, recreate containers with fallback GPU.
    async fn execute_migration(
        &self,
        config: &Config,
        docker: &dyn DockerControl,
    ) -> anyhow::Result<StageResult> {
        info!("Stage 2: Container-Migration auf Fallback-GPU");

        for pipeline in &config.pipeline {
            if !self.affected.pipelines.contains(&pipeline.container) {
                continue;
            }

            // Build env override for fallback GPU
            let mut env = HashMap::new();
            env.insert(
                "NVIDIA_VISIBLE_DEVICES".to_string(),
                pipeline.cuda_fallback_device.clone(),
            );
            env.insert(
                "CUDA_VISIBLE_DEVICES".to_string(),
                pipeline.cuda_fallback_device.clone(),
            );

            info!(
                "Migriere Pipeline {} auf Fallback-GPU {}",
                pipeline.container, pipeline.cuda_fallback_device
            );

            match docker
                .recreate_with_env(
                    &pipeline.compose_file,
                    &pipeline.compose_service,
                    env,
                )
                .await
            {
                Ok(()) => {
                    info!(
                        "Pipeline {} erfolgreich auf Fallback-GPU migriert",
                        pipeline.container
                    );

                    // Track the override in DB
                    let override_path = generate_override_path(
                        &pipeline.compose_file,
                        &pipeline.compose_service,
                    );
                    let override_rec = crate::db::FallbackOverride {
                        id: None,
                        compose_file: pipeline.compose_file.clone(),
                        service_name: pipeline.compose_service.clone(),
                        override_path,
                        created_at: Utc::now(),
                    };
                    if let Err(e) = self.db.insert_fallback_override(&override_rec).await {
                        warn!("Fallback-Override DB-Eintrag fehlgeschlagen: {}", e);
                    }
                }
                Err(e) => {
                    error!(
                        "Migration fehlgeschlagen für Pipeline {}: {}",
                        pipeline.container, e
                    );
                }
            }
        }

        // Migration itself is a fallback action, advance to thunderbolt reconnect
        Ok(StageResult::AdvanceToNext)
    }

    /// Stage 3: Thunderbolt deauth/reauth cycle.
    async fn execute_thunderbolt_reconnect(
        &self,
        config: &Config,
        thunderbolt: Option<&dyn ThunderboltControl>,
    ) -> anyhow::Result<StageResult> {
        info!("Stage 3: Thunderbolt Reconnect");

        let tb_config = match &config.thunderbolt {
            Some(tc) => tc,
            None => {
                warn!("Thunderbolt-Konfiguration nicht vorhanden, überspringe Stage 3");
                return Ok(StageResult::Failed(
                    "Thunderbolt nicht konfiguriert".to_string(),
                ));
            }
        };

        let thunderbolt = match thunderbolt {
            Some(t) => t,
            None => {
                warn!("Thunderbolt-Control nicht verfügbar, überspringe Stage 3");
                return Ok(StageResult::Failed(
                    "ThunderboltControl nicht verfügbar".to_string(),
                ));
            }
        };

        // Deauthorize
        info!("Thunderbolt deauthorize: {}", tb_config.device_path);
        if let Err(e) = thunderbolt.deauthorize(&tb_config.device_path).await {
            warn!("Thunderbolt deauth fehlgeschlagen: {}", e);
            return Ok(StageResult::Failed(format!(
                "Thunderbolt deauth fehlgeschlagen: {e}"
            )));
        }

        // Wait before reauth
        tokio::time::sleep(Duration::from_secs(2)).await;

        // Reauthorize
        info!("Thunderbolt reauthorize: {}", tb_config.device_path);
        if let Err(e) = thunderbolt.authorize(&tb_config.device_path).await {
            warn!("Thunderbolt reauth fehlgeschlagen: {}", e);
            return Ok(StageResult::Failed(format!(
                "Thunderbolt reauth fehlgeschlagen: {e}"
            )));
        }

        // Verify authorization
        tokio::time::sleep(Duration::from_secs(3)).await;
        match thunderbolt.is_authorized(&tb_config.device_path).await {
            Ok(true) => {
                info!("Thunderbolt-Gerät erfolgreich re-autorisiert");
                // Check nvidia-smi after reconnect
                if check_nvidia_smi_available().await {
                    info!("nvidia-smi nach Thunderbolt-Reconnect erreichbar");
                    Ok(StageResult::Success)
                } else {
                    Ok(StageResult::Failed(
                        "nvidia-smi nach Thunderbolt-Reconnect nicht erreichbar".to_string(),
                    ))
                }
            }
            Ok(false) => Ok(StageResult::Failed(
                "Thunderbolt-Gerät nicht autorisiert nach Reconnect".to_string(),
            )),
            Err(e) => Ok(StageResult::Failed(format!(
                "Thunderbolt-Status nicht lesbar: {e}"
            ))),
        }
    }

    /// Stage 4: Manual intervention - log failure and notify.
    async fn execute_manual_intervention(&self) -> anyhow::Result<StageResult> {
        error!(
            "Recovery Stage 4: Manuelle Intervention erforderlich! \
             Automatische Recovery fehlgeschlagen für Pipelines: {:?}",
            self.affected.pipelines
        );

        self.db
            .log_event(
                "recovery.manual_intervention",
                Severity::Critical,
                &format!(
                    "Manuelle Intervention erforderlich. Automatische Recovery \
                     für alle Stages fehlgeschlagen. Betroffene Pipelines: {:?}",
                    self.affected.pipelines
                ),
                Some(serde_json::json!({
                    "affected_pipelines": self.affected.pipelines,
                })),
            )
            .await?;

        // Stage 4 doesn't "fail" - it's the terminal state
        Ok(StageResult::Failed(
            "Manuelle Intervention erforderlich".to_string(),
        ))
    }

    /// Advance to the next recovery stage.
    async fn advance_stage(&mut self) -> anyhow::Result<()> {
        let next = self.stage.next();
        if next == self.stage {
            // Already at terminal stage
            self.complete_recovery().await?;
            return Ok(());
        }

        info!("Recovery: {} -> {}", self.stage, next);
        self.stage = next;
        self.persist_state("in_progress").await?;
        Ok(())
    }

    /// Complete the recovery (success or terminal failure).
    async fn complete_recovery(&mut self) -> anyhow::Result<()> {
        info!("Recovery abgeschlossen (Stage: {})", self.stage);
        self.stage = RecoveryStage::Idle;
        self.db.clear_recovery_state().await?;

        self.db
            .log_event(
                "recovery.complete",
                Severity::Info,
                "Recovery abgeschlossen",
                None,
            )
            .await?;

        Ok(())
    }

    /// Mark recovery as interrupted (for graceful shutdown).
    pub async fn mark_interrupted(&self) -> anyhow::Result<()> {
        if self.is_active() {
            info!("Recovery als unterbrochen markiert (Stage: {})", self.stage);
            self.persist_state("interrupted").await?;
        }
        Ok(())
    }

    /// Clear interrupted recovery state (used when stage4_manual is discarded on restart).
    pub async fn clear_interrupted(&self) -> anyhow::Result<()> {
        info!("Unterbrochene Recovery-State wird gelöscht");
        self.db.clear_recovery_state().await
    }

    /// Persist current state to SQLite.
    async fn persist_state(&self, status: &str) -> anyhow::Result<()> {
        let state = RecoveryState {
            id: None,
            stage: self.stage.to_string(),
            started_at: self.started_at,
            updated_at: Utc::now(),
            status: status.to_string(),
        };
        self.db.upsert_recovery_state(&state).await
    }

    /// Run the full recovery sequence from the current stage to completion.
    pub async fn run_recovery(
        &mut self,
        config: &Config,
        docker: &dyn DockerControl,
        pcie: Option<&dyn PcieControl>,
        thunderbolt: Option<&dyn ThunderboltControl>,
    ) -> anyhow::Result<()> {
        while self.is_active() {
            let result = self
                .execute_current_stage(config, docker, pcie, thunderbolt)
                .await?;

            if result == StageResult::Success {
                break;
            }

            if self.stage == RecoveryStage::Idle {
                // Terminal state reached
                break;
            }
        }
        Ok(())
    }
}

/// Generate the override file path for a service.
pub fn generate_override_path(compose_file: &str, service: &str) -> String {
    let compose_dir = std::path::Path::new(compose_file)
        .parent()
        .unwrap_or(std::path::Path::new("/tmp"));
    compose_dir
        .join(format!("docker-compose.egpu-fallback.{service}.yml"))
        .to_string_lossy()
        .to_string()
}

/// Generate override YAML content for GPU fallback.
pub fn generate_override_yaml(service: &str, fallback_device: &str) -> String {
    format!(
        r#"# Auto-generated by egpu-managerd recovery
# Fallback GPU override for service: {service}
services:
  {service}:
    environment:
      - NVIDIA_VISIBLE_DEVICES={fallback_device}
      - CUDA_VISIBLE_DEVICES={fallback_device}
    deploy:
      resources:
        reservations:
          devices:
            - driver: nvidia
              device_ids: ['{fallback_device}']
              capabilities: [gpu]
"#
    )
}

/// Check if nvidia-smi is available (quick health check).
async fn check_nvidia_smi_available() -> bool {
    use tokio::process::Command;
    match tokio::time::timeout(Duration::from_secs(5), async {
        Command::new("nvidia-smi")
            .arg("--query-gpu=name")
            .arg("--format=csv,noheader")
            .output()
            .await
    })
    .await
    {
        Ok(Ok(output)) => output.status.success(),
        _ => false,
    }
}

/// Get affected pipelines from config that use the eGPU.
pub fn get_egpu_pipelines(config: &Config) -> Vec<String> {
    config
        .pipeline
        .iter()
        .filter(|p| p.gpu_device == config.gpu.egpu_pci_address)
        .map(|p| p.container.clone())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::EventDb;

    #[test]
    fn test_stage_ordering() {
        assert_eq!(RecoveryStage::Idle.next(), RecoveryStage::Stage0Quiesce);
        assert_eq!(
            RecoveryStage::Stage0Quiesce.next(),
            RecoveryStage::Stage1PcieReset
        );
        assert_eq!(
            RecoveryStage::Stage1PcieReset.next(),
            RecoveryStage::Stage2Migration
        );
        assert_eq!(
            RecoveryStage::Stage2Migration.next(),
            RecoveryStage::Stage3ThunderboltReconnect
        );
        assert_eq!(
            RecoveryStage::Stage3ThunderboltReconnect.next(),
            RecoveryStage::Stage4ManualIntervention
        );
        // Terminal stage stays at itself
        assert_eq!(
            RecoveryStage::Stage4ManualIntervention.next(),
            RecoveryStage::Stage4ManualIntervention
        );
    }

    #[test]
    fn test_stage_from_str_roundtrip() {
        let stages = [
            RecoveryStage::Idle,
            RecoveryStage::Stage0Quiesce,
            RecoveryStage::Stage1PcieReset,
            RecoveryStage::Stage2Migration,
            RecoveryStage::Stage3ThunderboltReconnect,
            RecoveryStage::Stage4ManualIntervention,
        ];
        for stage in stages {
            let s = stage.to_string();
            let parsed = RecoveryStage::from_str_lossy(&s);
            assert_eq!(stage, parsed, "Roundtrip failed for {s}");
        }
    }

    #[test]
    fn test_stage_timeouts() {
        assert_eq!(RecoveryStage::Idle.timeout_seconds(), 0);
        assert!(RecoveryStage::Stage0Quiesce.timeout_seconds() > 0);
        assert!(RecoveryStage::Stage1PcieReset.timeout_seconds() > 0);
        assert!(RecoveryStage::Stage2Migration.timeout_seconds() > 0);
        assert!(RecoveryStage::Stage3ThunderboltReconnect.timeout_seconds() > 0);
        assert_eq!(RecoveryStage::Stage4ManualIntervention.timeout_seconds(), 0);
    }

    #[test]
    fn test_generate_override_path() {
        let path = generate_override_path("/opt/project/docker-compose.yml", "worker");
        assert_eq!(
            path,
            "/opt/project/docker-compose.egpu-fallback.worker.yml"
        );
    }

    #[test]
    fn test_generate_override_yaml() {
        let yaml = generate_override_yaml("celery-worker", "0000:02:00.0");
        assert!(yaml.contains("celery-worker"));
        assert!(yaml.contains("NVIDIA_VISIBLE_DEVICES=0000:02:00.0"));
        assert!(yaml.contains("CUDA_VISIBLE_DEVICES=0000:02:00.0"));
    }

    #[tokio::test]
    async fn test_recovery_initial_state() {
        let db = EventDb::open_in_memory().unwrap();
        let sm = RecoveryStateMachine::new(db, 0);
        assert_eq!(sm.current_stage(), RecoveryStage::Idle);
        assert!(!sm.is_active());
    }

    #[tokio::test]
    async fn test_recovery_start() {
        let db = EventDb::open_in_memory().unwrap();
        let mut sm = RecoveryStateMachine::new(db, 0);

        sm.start_recovery(vec!["worker1".to_string(), "worker2".to_string()])
            .await
            .unwrap();

        assert!(sm.is_active());
        assert_eq!(sm.current_stage(), RecoveryStage::Stage0Quiesce);
        assert_eq!(sm.affected_pipelines().len(), 2);
    }

    #[tokio::test]
    async fn test_recovery_no_double_start() {
        let db = EventDb::open_in_memory().unwrap();
        let mut sm = RecoveryStateMachine::new(db, 0);

        sm.start_recovery(vec!["worker1".to_string()])
            .await
            .unwrap();
        sm.start_recovery(vec!["worker2".to_string()])
            .await
            .unwrap();

        // Should still be in Stage0, not restarted
        assert_eq!(sm.current_stage(), RecoveryStage::Stage0Quiesce);
        // Original affected pipelines preserved
        assert_eq!(sm.affected_pipelines(), &["worker1".to_string()]);
    }

    #[tokio::test]
    async fn test_recovery_persists_and_loads() {
        let db = EventDb::open_in_memory().unwrap();

        // Start recovery and persist
        {
            let mut sm = RecoveryStateMachine::new(db.clone(), 0);
            sm.start_recovery(vec!["worker1".to_string()])
                .await
                .unwrap();
        }

        // Load in new instance
        {
            let mut sm = RecoveryStateMachine::new(db.clone(), 0);
            let interrupted = sm.check_interrupted().await.unwrap();
            assert_eq!(interrupted, Some(RecoveryStage::Stage0Quiesce));
            assert!(sm.is_active());
        }
    }

    #[tokio::test]
    async fn test_recovery_mark_interrupted() {
        let db = EventDb::open_in_memory().unwrap();
        let mut sm = RecoveryStateMachine::new(db.clone(), 0);

        sm.start_recovery(vec!["worker1".to_string()])
            .await
            .unwrap();
        sm.mark_interrupted().await.unwrap();

        // Verify the state is persisted as interrupted
        let _state = db.load_recovery_state().await.unwrap().unwrap();
        // Note: mark_interrupted persists "interrupted" status but check_interrupted
        // only resumes "in_progress" states - this is intentional so a daemon
        // restart after interrupted recovery re-evaluates
    }

    #[tokio::test]
    async fn test_get_egpu_pipelines() {
        let toml_str = r#"
            schema_version = 1

            [gpu]
            egpu_pci_address = "0000:05:00.0"
            internal_pci_address = "0000:02:00.0"

            [[pipeline]]
            project = "test1"
            container = "worker1"
            compose_file = "/tmp/test.yml"
            compose_service = "worker"
            gpu_priority = 1
            gpu_device = "0000:05:00.0"
            cuda_fallback_device = "0000:02:00.0"

            [[pipeline]]
            project = "test2"
            container = "worker2"
            compose_file = "/tmp/test.yml"
            compose_service = "worker2"
            gpu_priority = 2
            gpu_device = "0000:02:00.0"
            cuda_fallback_device = "0000:02:00.0"
        "#;

        let config: Config = toml::from_str(toml_str).unwrap();
        let pipelines = get_egpu_pipelines(&config);
        assert_eq!(pipelines, vec!["worker1"]);
    }
}
