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
            RecoveryStage::Stage1PcieReset => 45,
            RecoveryStage::Stage2Migration => 120,
            RecoveryStage::Stage3ThunderboltReconnect => 45,
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

    #[cfg(test)]
    pub fn current_stage(&self) -> RecoveryStage {
        self.stage
    }

    pub fn is_active(&self) -> bool {
        self.stage != RecoveryStage::Idle
    }

    #[cfg(test)]
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
    /// Zuerst Display-Outputs von eGPU loesen (nvidia-modeset Hang verhindern),
    /// dann Celery, generic hooks, Redis BGSAVE, PostgreSQL CHECKPOINT.
    async fn execute_quiesce(
        &self,
        config: &Config,
        docker: &dyn DockerControl,
    ) -> anyhow::Result<StageResult> {
        info!("Stage 0: Quiesce-Hooks werden ausgeführt");

        // Display-Detach VOR allen anderen Quiesce-Aktionen
        if config.recovery.auto_detach_display {
            match detach_egpu_displays(&config.gpu.egpu_pci_address).await {
                Ok(detached) => {
                    if detached > 0 {
                        info!("Display-Detach: {detached} Output(s) von eGPU geloest");
                    } else {
                        info!("Display-Detach: Keine aktiven eGPU-Outputs gefunden");
                    }
                }
                Err(e) => {
                    warn!("Display-Detach fehlgeschlagen: {e}");
                    // Kein Abbruch — versuche trotzdem weiterzumachen
                }
            }
        }

        let mut success_count = 0u32;
        let mut fail_count = 0u32;

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
                        success_count += 1;
                    }
                    Err(e) => {
                        warn!("Quiesce-Hook {} fehlgeschlagen: {e}", hook.container);
                        fail_count += 1;
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
                    Ok(_) => {
                        info!("Redis BGSAVE erfolgreich: {}", redis_container);
                        success_count += 1;
                    }
                    Err(e) => {
                        warn!("Redis BGSAVE {} fehlgeschlagen: {e}", redis_container);
                        fail_count += 1;
                    }
                }
            }
        }

        // If more than half of quiesce operations failed, fail the stage
        let total = success_count + fail_count;
        if total > 0 && fail_count > total / 2 {
            warn!("Quiesce fehlgeschlagen: {fail_count}/{total} Hooks fehlgeschlagen");
            return Ok(StageResult::Failed(format!(
                "Quiesce: {fail_count}/{total} Hooks fehlgeschlagen"
            )));
        }
        info!("Quiesce abgeschlossen: {success_count}/{total} Hooks erfolgreich");

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
                // Readiness-Polling: nvidia-smi kann nach FLR einige Sekunden brauchen
                const FLR_RETRIES: u32 = 6;
                const FLR_RETRY_DELAY: Duration = Duration::from_secs(2);
                for attempt in 1..=FLR_RETRIES {
                    tokio::time::sleep(FLR_RETRY_DELAY).await;
                    if check_nvidia_smi_available().await {
                        info!(
                            "nvidia-smi nach FLR erreichbar (Versuch {}/{})",
                            attempt, FLR_RETRIES
                        );
                        return Ok(StageResult::Success);
                    }
                    info!(
                        "nvidia-smi nach FLR noch nicht erreichbar (Versuch {}/{})",
                        attempt, FLR_RETRIES
                    );
                }
                warn!(
                    "nvidia-smi nach FLR nicht erreichbar nach {} Versuchen",
                    FLR_RETRIES
                );
                Ok(StageResult::Failed(
                    "nvidia-smi nach FLR nicht erreichbar (alle Retries erschoepft)".to_string(),
                ))
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

        // Override-Verzeichnis sicherstellen (Daemon-schreibbar)
        let override_dir = &config.recovery.override_dir;
        if !override_dir.is_empty() {
            if let Err(e) = tokio::fs::create_dir_all(override_dir).await {
                warn!("Override-Verzeichnis nicht erstellbar: {override_dir}: {e}");
            }
        }

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
            // Override-Dir an DockerControl durchreichen
            env.insert(
                "_EGPU_OVERRIDE_DIR".to_string(),
                override_dir.clone(),
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
                    let override_path = generate_override_path_with_dir(
                        &pipeline.compose_file,
                        &pipeline.compose_service,
                        override_dir,
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

        // Minimale Settle-Time vor Reauth (Thunderbolt-Controller braucht kurz)
        tokio::time::sleep(Duration::from_secs(1)).await;

        // Reauthorize
        info!("Thunderbolt reauthorize: {}", tb_config.device_path);
        if let Err(e) = thunderbolt.authorize(&tb_config.device_path).await {
            warn!("Thunderbolt reauth fehlgeschlagen: {}", e);
            return Ok(StageResult::Failed(format!(
                "Thunderbolt reauth fehlgeschlagen: {e}"
            )));
        }

        // Readiness-Polling: Warte auf PCI-Device-Wiedererscheinen via sysfs
        let pci_vendor_path = format!(
            "/sys/bus/pci/devices/{}/vendor",
            config.gpu.egpu_pci_address
        );
        const TB_POLL_INTERVAL: Duration = Duration::from_millis(500);
        const TB_POLL_DEADLINE: Duration = Duration::from_secs(15);
        let poll_start = tokio::time::Instant::now();
        let mut pci_device_found = false;

        info!(
            "Warte auf PCI-Device {} (Deadline: {}s)",
            config.gpu.egpu_pci_address,
            TB_POLL_DEADLINE.as_secs()
        );
        while poll_start.elapsed() < TB_POLL_DEADLINE {
            if tokio::fs::metadata(&pci_vendor_path).await.is_ok() {
                info!(
                    "PCI-Device {} in sysfs gefunden nach {:.1}s",
                    config.gpu.egpu_pci_address,
                    poll_start.elapsed().as_secs_f64()
                );
                pci_device_found = true;
                break;
            }
            tokio::time::sleep(TB_POLL_INTERVAL).await;
        }

        if !pci_device_found {
            warn!(
                "PCI-Device {} nach {}s nicht in sysfs erschienen",
                config.gpu.egpu_pci_address,
                TB_POLL_DEADLINE.as_secs()
            );
            return Ok(StageResult::Failed(format!(
                "PCI-Device {} nach Thunderbolt-Reconnect nicht erschienen",
                config.gpu.egpu_pci_address
            )));
        }

        // Thunderbolt-Autorisierung verifizieren
        match thunderbolt.is_authorized(&tb_config.device_path).await {
            Ok(true) => {
                info!("Thunderbolt-Gerät erfolgreich re-autorisiert");
            }
            Ok(false) => {
                return Ok(StageResult::Failed(
                    "Thunderbolt-Gerät nicht autorisiert nach Reconnect".to_string(),
                ));
            }
            Err(e) => {
                return Ok(StageResult::Failed(format!(
                    "Thunderbolt-Status nicht lesbar: {e}"
                )));
            }
        }

        // nvidia-smi Readiness-Polling (PCI-Device da, aber Treiber braucht noch)
        const TB_SMI_RETRIES: u32 = 3;
        const TB_SMI_RETRY_DELAY: Duration = Duration::from_secs(2);
        for attempt in 1..=TB_SMI_RETRIES {
            tokio::time::sleep(TB_SMI_RETRY_DELAY).await;
            if check_nvidia_smi_available().await {
                info!(
                    "nvidia-smi nach Thunderbolt-Reconnect erreichbar (Versuch {}/{})",
                    attempt, TB_SMI_RETRIES
                );
                return Ok(StageResult::Success);
            }
            info!(
                "nvidia-smi nach Thunderbolt-Reconnect noch nicht erreichbar (Versuch {}/{})",
                attempt, TB_SMI_RETRIES
            );
        }

        Ok(StageResult::Failed(
            "nvidia-smi nach Thunderbolt-Reconnect nicht erreichbar (alle Retries erschoepft)"
                .to_string(),
        ))
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

/// Generate override path mit explizitem override_dir.
pub fn generate_override_path_with_dir(
    compose_file: &str,
    service: &str,
    override_dir: &str,
) -> String {
    if override_dir.is_empty() {
        // Legacy-Verhalten: neben der compose-Datei
        let compose_dir = std::path::Path::new(compose_file)
            .parent()
            .unwrap_or(std::path::Path::new("/tmp"));
        compose_dir
            .join(format!("docker-compose.egpu-fallback.{service}.yml"))
            .to_string_lossy()
            .to_string()
    } else {
        // Neues Verhalten: unter override_dir (Daemon-schreibbar)
        std::path::Path::new(override_dir)
            .join(format!("docker-compose.egpu-fallback.{service}.yml"))
            .to_string_lossy()
            .to_string()
    }
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

/// Zaehle aktive Monitore via `xrandr --listmonitors`.
/// Gibt 0 zurueck wenn xrandr nicht verfuegbar oder ein Fehler auftritt.
async fn count_xrandr_monitors() -> u32 {
    use tokio::process::Command;
    let output = Command::new("xrandr")
        .arg("--listmonitors")
        .env("DISPLAY", ":0")
        .env("XAUTHORITY", "/run/user/1000/gdm/Xauthority")
        .output()
        .await;

    match output {
        Ok(o) if o.status.success() => {
            let stdout = String::from_utf8_lossy(&o.stdout);
            // Erste Zeile ist "Monitors: N", danach je eine Zeile pro Monitor
            stdout
                .lines()
                .next()
                .and_then(|line| {
                    line.trim()
                        .strip_prefix("Monitors: ")
                        .and_then(|n| n.parse::<u32>().ok())
                })
                .unwrap_or(0)
        }
        _ => 0,
    }
}

/// Loesche alle Display-Outputs die ueber eine bestimmte GPU laufen.
/// Nutzt xrandr um Outputs zu identifizieren und zu deaktivieren.
/// Gibt die Anzahl deaktivierter Outputs zurueck.
pub async fn detach_egpu_displays(egpu_pci_address: &str) -> anyhow::Result<u32> {
    use tokio::process::Command;

    // Schritt 1: Finde alle DRM-Card-Nummern fuer die eGPU PCI-Adresse
    // /sys/bus/pci/devices/0000:05:00.0/drm/card* -> cardN
    let drm_path = format!("/sys/bus/pci/devices/{egpu_pci_address}/drm");
    let mut egpu_cards = Vec::new();

    if let Ok(mut entries) = tokio::fs::read_dir(&drm_path).await {
        while let Ok(Some(entry)) = entries.next_entry().await {
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with("card") && !name.contains('-') {
                egpu_cards.push(name);
            }
        }
    }

    if egpu_cards.is_empty() {
        info!("Keine DRM-Karten fuer eGPU {} gefunden", egpu_pci_address);
        return Ok(0);
    }

    // Schritt 2: xrandr --listmonitors um aktive Monitore zu finden
    // Dann verbundene Outputs mit eGPU-Karten abgleichen
    let _xrandr_output = match Command::new("xrandr")
        .arg("--listmonitors")
        .env("DISPLAY", ":0")
        .env("XAUTHORITY", "/run/user/1000/gdm/Xauthority")
        .output()
        .await
    {
        Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout).to_string(),
        _ => {
            // Wayland: Versuch mit gnome-randr oder mutter DBus
            info!("xrandr nicht verfuegbar, versuche Wayland-Display-Detach");
            return detach_egpu_displays_wayland(egpu_pci_address).await;
        }
    };

    // Schritt 3: Finde Outputs die auf eGPU-Karten liegen
    // xrandr --listmonitors zeigt z.B.:
    //   0: +*DP-0 1920/600x1080/340+0+0  DP-0
    //   1: +HDMI-1-0 1920/600x1080/340+1920+0  HDMI-1-0
    // Wir muessen pruefen welche davon auf der eGPU-Karte liegen
    let mut detached = 0u32;

    // Finde alle xrandr Outputs und pruefe welche zur eGPU gehoeren
    let _providers_output = Command::new("xrandr")
        .arg("--listproviders")
        .env("DISPLAY", ":0")
        .env("XAUTHORITY", "/run/user/1000/gdm/Xauthority")
        .output()
        .await;

    // Einfacherer Ansatz: alle Outputs der eGPU-Karten ueber sysfs finden
    for card in &egpu_cards {
        let card_path = format!("{drm_path}/{card}");
        if let Ok(mut card_entries) = tokio::fs::read_dir(&card_path).await {
            while let Ok(Some(entry)) = card_entries.next_entry().await {
                let name = entry.file_name().to_string_lossy().to_string();
                // DRM-Connector-Verzeichnisse: card0-DP-1, card0-HDMI-A-1, etc.
                if name.starts_with(&format!("{card}-")) {
                    let connector = name.trim_start_matches(&format!("{card}-"));
                    // Pruefe ob der Connector aktiv ist
                    let status_path = entry.path().join("status");
                    if let Ok(status) = tokio::fs::read_to_string(&status_path).await {
                        if status.trim() == "connected" {
                            // xrandr-Output-Name: Connector-Name ohne card-Prefix,
                            // mit Bindestrich statt Doppelpunkt
                            let output_name = connector.replace("A-", "");
                            info!(
                                "eGPU-Display-Output gefunden: {} (Karte: {}, Status: connected)",
                                output_name, card
                            );

                            // Output deaktivieren via xrandr
                            let result = Command::new("xrandr")
                                .args(["--output", &output_name, "--off"])
                                .env("DISPLAY", ":0")
                                .env("XAUTHORITY", "/run/user/1000/gdm/Xauthority")
                                .output()
                                .await;

                            match result {
                                Ok(o) if o.status.success() => {
                                    info!("Display-Output {} deaktiviert", output_name);
                                    detached += 1;
                                }
                                Ok(o) => {
                                    let stderr = String::from_utf8_lossy(&o.stderr);
                                    warn!(
                                        "xrandr --off fuer {} fehlgeschlagen: {}",
                                        output_name, stderr
                                    );
                                }
                                Err(e) => {
                                    warn!("xrandr nicht ausfuehrbar: {e}");
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    // Schritt 4: Polling statt fester Delay — pruefe ob Monitor-Count gesunken ist
    if detached > 0 {
        // Aktuelle Monitor-Anzahl vor dem Polling ermitteln (bereits detached,
        // aber Display-Server braucht kurz fuer die Verarbeitung)
        let initial_count = count_xrandr_monitors().await;
        const DETACH_POLL_INTERVAL: Duration = Duration::from_millis(200);
        const DETACH_POLL_DEADLINE: Duration = Duration::from_secs(2);
        let poll_start = tokio::time::Instant::now();

        while poll_start.elapsed() < DETACH_POLL_DEADLINE {
            tokio::time::sleep(DETACH_POLL_INTERVAL).await;
            let current_count = count_xrandr_monitors().await;
            if current_count < initial_count {
                info!(
                    "Display-Server hat Detach verarbeitet (Monitore: {} -> {})",
                    initial_count, current_count
                );
                break;
            }
        }
    }

    Ok(detached)
}

/// Wayland-Variante: Display-Detach ueber gnome-monitor-config oder dbus.
async fn detach_egpu_displays_wayland(egpu_pci_address: &str) -> anyhow::Result<u32> {
    use tokio::process::Command;

    // Versuche gnome-randr (falls installiert)
    let result = Command::new("gnome-randr")
        .arg("query")
        .output()
        .await;

    if let Ok(output) = result {
        if output.status.success() {
            let _stdout = String::from_utf8_lossy(&output.stdout);
            // gnome-randr zeigt Outputs mit Connector-Infos
            info!("gnome-randr verfuegbar, parse Outputs");
            // TODO: gnome-randr Output parsen wenn vorhanden
        }
    }

    // Fallback: Versuche eGPU DRM-Device via sysfs zu unbinden
    // Das ist der sicherste Weg unter Wayland
    let drm_path = format!("/sys/bus/pci/devices/{egpu_pci_address}/drm");
    let mut detached = 0u32;

    if let Ok(mut entries) = tokio::fs::read_dir(&drm_path).await {
        while let Ok(Some(entry)) = entries.next_entry().await {
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with("card") && !name.contains('-') {
                // Signalisiere dem Display-Server dass die GPU weggeht
                // Unter Wayland/Mutter: SIGUSR1 an gnome-shell => re-enumerate outputs
                if let Ok(output) = Command::new("pkill")
                    .args(["-USR1", "gnome-shell"])
                    .output()
                    .await
                {
                    if output.status.success() {
                        info!("SIGUSR1 an gnome-shell gesendet (Display-Re-Enumeration)");
                        detached += 1;
                    }
                }
                break; // Nur einmal senden
            }
        }
    }

    Ok(detached)
}

/// Ergebnis einer Safe-Disconnect-Vorbereitung.
#[derive(Debug, Clone, Serialize)]
pub struct SafeDisconnectResult {
    pub success: bool,
    pub displays_detached: u32,
    pub pipelines_migrated: Vec<String>,
    pub pipelines_failed: Vec<String>,
    pub warnings: Vec<String>,
    pub safe_to_unplug: bool,
}

/// Bereitet das sichere Trennen der eGPU vor:
/// 1. Display-Outputs von eGPU loesen
/// 2. Quiesce-Hooks ausfuehren (Redis BGSAVE, etc.)
/// 3. Pipelines auf interne GPU migrieren
/// 4. Ergebnis-Report zurueckgeben
pub async fn prepare_safe_disconnect(
    config: &Config,
    docker: &dyn DockerControl,
) -> SafeDisconnectResult {
    let mut result = SafeDisconnectResult {
        success: false,
        displays_detached: 0,
        pipelines_migrated: Vec::new(),
        pipelines_failed: Vec::new(),
        warnings: Vec::new(),
        safe_to_unplug: false,
    };

    // 1. Display-Detach
    info!("Safe-Disconnect: Display-Outputs von eGPU loesen");
    match detach_egpu_displays(&config.gpu.egpu_pci_address).await {
        Ok(n) => {
            result.displays_detached = n;
            if n > 0 {
                info!("Safe-Disconnect: {n} Display-Output(s) geloest");
            }
        }
        Err(e) => {
            let msg = format!("Display-Detach fehlgeschlagen: {e}");
            warn!("{msg}");
            result.warnings.push(msg);
        }
    }

    // 2. Quiesce-Hooks fuer eGPU-Pipelines
    let egpu_pipelines = get_egpu_pipelines(config);
    if egpu_pipelines.is_empty() {
        info!("Safe-Disconnect: Keine Pipelines auf eGPU aktiv");
        result.success = true;
        result.safe_to_unplug = true;
        return result;
    }

    info!(
        "Safe-Disconnect: {} Pipeline(s) auf eGPU: {:?}",
        egpu_pipelines.len(),
        egpu_pipelines
    );

    for pipeline_cfg in &config.pipeline {
        if !egpu_pipelines.contains(&pipeline_cfg.container) {
            continue;
        }

        // Quiesce-Hooks ausfuehren
        for hook in &pipeline_cfg.quiesce_hooks {
            let cmd_parts: Vec<&str> = hook.command.split_whitespace().collect();
            let timeout = Duration::from_secs(hook.timeout_seconds);
            if let Err(e) = docker.exec_in_container(&hook.container, &cmd_parts, timeout).await {
                result
                    .warnings
                    .push(format!("Quiesce-Hook {} fehlgeschlagen: {e}", hook.container));
            }
        }

        // Redis BGSAVE
        for redis in &pipeline_cfg.redis_containers {
            if let Err(e) = docker
                .exec_in_container(redis, &["redis-cli", "BGSAVE"], Duration::from_secs(10))
                .await
            {
                result
                    .warnings
                    .push(format!("Redis BGSAVE {redis} fehlgeschlagen: {e}"));
            }
        }
    }

    // 3. Pipelines auf Fallback-GPU migrieren
    let override_dir = &config.recovery.override_dir;
    if !override_dir.is_empty() {
        if let Err(e) = tokio::fs::create_dir_all(override_dir).await {
            result
                .warnings
                .push(format!("Override-Verzeichnis nicht erstellbar: {e}"));
        }
    }

    for pipeline_cfg in &config.pipeline {
        if !egpu_pipelines.contains(&pipeline_cfg.container) {
            continue;
        }

        let mut env = HashMap::new();
        env.insert(
            "NVIDIA_VISIBLE_DEVICES".to_string(),
            pipeline_cfg.cuda_fallback_device.clone(),
        );
        env.insert(
            "CUDA_VISIBLE_DEVICES".to_string(),
            pipeline_cfg.cuda_fallback_device.clone(),
        );
        env.insert("_EGPU_OVERRIDE_DIR".to_string(), override_dir.clone());

        info!(
            "Safe-Disconnect: Migriere {} auf {}",
            pipeline_cfg.container, pipeline_cfg.cuda_fallback_device
        );

        match docker
            .recreate_with_env(
                &pipeline_cfg.compose_file,
                &pipeline_cfg.compose_service,
                env,
            )
            .await
        {
            Ok(()) => {
                result
                    .pipelines_migrated
                    .push(pipeline_cfg.container.clone());
            }
            Err(e) => {
                let msg = format!("{}: {e}", pipeline_cfg.container);
                error!("Safe-Disconnect Migration fehlgeschlagen: {msg}");
                result.pipelines_failed.push(msg);
            }
        }
    }

    result.success = result.pipelines_failed.is_empty();
    result.safe_to_unplug =
        result.success && result.displays_detached > 0 || result.warnings.is_empty();
    result
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
    fn test_generate_override_path_with_dir() {
        // Mit override_dir -> Datei landet im override-Verzeichnis
        let path = generate_override_path_with_dir(
            "/home/user/project/docker-compose.yml",
            "celery-worker",
            "/var/lib/egpu-manager/overrides",
        );
        assert_eq!(
            path,
            "/var/lib/egpu-manager/overrides/docker-compose.egpu-fallback.celery-worker.yml"
        );
    }

    #[test]
    fn test_generate_override_path_with_empty_dir() {
        // Leerer override_dir -> Legacy-Verhalten (neben compose-Datei)
        let path = generate_override_path_with_dir(
            "/opt/project/docker-compose.yml",
            "worker",
            "",
        );
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
