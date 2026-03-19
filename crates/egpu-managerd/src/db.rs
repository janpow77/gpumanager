use std::path::{Path, PathBuf};
use std::sync::Arc;

use chrono::{DateTime, Utc};
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;
use tracing::info;

/// Event severity levels
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Debug,
    Info,
    Warning,
    Error,
    Critical,
}

impl std::fmt::Display for Severity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Severity::Debug => write!(f, "debug"),
            Severity::Info => write!(f, "info"),
            Severity::Warning => write!(f, "warning"),
            Severity::Error => write!(f, "error"),
            Severity::Critical => write!(f, "critical"),
        }
    }
}

impl Severity {
    pub fn from_str_lossy(s: &str) -> Self {
        match s {
            "debug" => Severity::Debug,
            "info" => Severity::Info,
            "warning" => Severity::Warning,
            "error" => Severity::Error,
            "critical" => Severity::Critical,
            _ => Severity::Info,
        }
    }
}

/// A stored event
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Event {
    pub id: Option<i64>,
    pub timestamp: DateTime<Utc>,
    pub event_type: String,
    pub severity: Severity,
    pub message: String,
    pub data: Option<serde_json::Value>,
}

/// Recovery state tracking
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecoveryState {
    pub id: Option<i64>,
    pub stage: String,
    pub started_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub status: String,
}

/// Fallback override tracking
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FallbackOverride {
    pub id: Option<i64>,
    pub compose_file: String,
    pub service_name: String,
    pub override_path: String,
    pub created_at: DateTime<Utc>,
}

/// Thread-safe database handle wrapping rusqlite (which is not Send).
/// All operations go through a Mutex-guarded connection.

/// Persistierter Lease fuer Daemon-Restart-Recovery.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersistedLease {
    pub lease_id: String,
    pub pipeline: String,
    pub gpu_device: String,
    pub workload_type: String,
    pub vram_mb: u64,
    pub acquired_at: String,
    pub expires_at: String,
    pub last_heartbeat: String,
}

#[derive(Clone)]
pub struct EventDb {
    conn: Arc<Mutex<Connection>>,
    db_path: Option<PathBuf>,
}

impl EventDb {
    /// Open or create the database at the given path.
    pub fn open(path: &Path) -> anyhow::Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let conn = Connection::open(path)?;

        // WAL-Modus fuer bessere Concurrent-Performance
        conn.execute_batch("PRAGMA journal_mode=WAL;")?;

        // Integritaetspruefung beim Start
        let integrity: String = conn.query_row("PRAGMA integrity_check;", [], |row| row.get(0))?;
        if integrity != "ok" {
            anyhow::bail!("SQLite Integritaetspruefung fehlgeschlagen: {}", integrity);
        }
        info!("SQLite: WAL-Modus aktiviert, Integritaet OK");

        Self::create_tables(&conn)?;
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
            db_path: Some(path.to_path_buf()),
        })
    }

    /// Open an in-memory database (for tests).
    #[cfg(test)]
    pub fn open_in_memory() -> anyhow::Result<Self> {
        let conn = Connection::open_in_memory()?;
        Self::create_tables(&conn)?;
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
            db_path: None,
        })
    }

    fn create_tables(conn: &Connection) -> anyhow::Result<()> {
        conn.execute_batch(
            "
            CREATE TABLE IF NOT EXISTS events (
                id          INTEGER PRIMARY KEY AUTOINCREMENT,
                timestamp   TEXT NOT NULL,
                event_type  TEXT NOT NULL,
                severity    TEXT NOT NULL,
                message     TEXT NOT NULL,
                data        TEXT
            );

            CREATE INDEX IF NOT EXISTS idx_events_timestamp ON events(timestamp);
            CREATE INDEX IF NOT EXISTS idx_events_type ON events(event_type);

            CREATE TABLE IF NOT EXISTS recovery_state (
                id          INTEGER PRIMARY KEY AUTOINCREMENT,
                stage       TEXT NOT NULL,
                started_at  TEXT NOT NULL,
                updated_at  TEXT NOT NULL,
                status      TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS fallback_overrides (
                id            INTEGER PRIMARY KEY AUTOINCREMENT,
                compose_file  TEXT NOT NULL,
                service_name  TEXT NOT NULL,
                override_path TEXT NOT NULL,
                created_at    TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS monitoring_aggregates (
                id            INTEGER PRIMARY KEY AUTOINCREMENT,
                bucket_start  TEXT NOT NULL,
                event_type    TEXT NOT NULL,
                count         INTEGER NOT NULL,
                avg_value     REAL,
                min_value     REAL,
                max_value     REAL
            );
            CREATE INDEX IF NOT EXISTS idx_agg_bucket ON monitoring_aggregates(bucket_start);

            CREATE TABLE IF NOT EXISTS gpu_telemetry (
                id                INTEGER PRIMARY KEY AUTOINCREMENT,
                timestamp         TEXT NOT NULL,
                pci_address       TEXT NOT NULL,
                gpu_type          TEXT NOT NULL,
                temperature_c     INTEGER NOT NULL,
                utilization_pct   INTEGER NOT NULL,
                memory_used_mb    INTEGER NOT NULL,
                memory_total_mb   INTEGER NOT NULL,
                power_draw_w      REAL NOT NULL,
                pstate            TEXT NOT NULL,
                fan_speed_pct     INTEGER NOT NULL DEFAULT 0,
                clock_graphics_mhz INTEGER NOT NULL DEFAULT 0,
                health_score      REAL,
                warning_level     TEXT
            );
            CREATE INDEX IF NOT EXISTS idx_telemetry_ts ON gpu_telemetry(timestamp);
            CREATE INDEX IF NOT EXISTS idx_telemetry_pci ON gpu_telemetry(pci_address);

            CREATE TABLE IF NOT EXISTS llm_usage (
                id            INTEGER PRIMARY KEY AUTOINCREMENT,
                app_id        TEXT NOT NULL,
                provider      TEXT NOT NULL,
                model         TEXT NOT NULL,
                input_tokens  INTEGER NOT NULL,
                output_tokens INTEGER NOT NULL,
                cost_usd      REAL NOT NULL,
                timestamp     TEXT NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_llm_usage_app ON llm_usage(app_id);
            CREATE INDEX IF NOT EXISTS idx_llm_usage_timestamp ON llm_usage(timestamp);

            CREATE TABLE IF NOT EXISTS active_leases (
                lease_id        TEXT PRIMARY KEY,
                pipeline        TEXT NOT NULL,
                gpu_device      TEXT NOT NULL,
                workload_type   TEXT NOT NULL,
                vram_mb         INTEGER NOT NULL,
                acquired_at     TEXT NOT NULL,
                expires_at      TEXT NOT NULL,
                last_heartbeat  TEXT NOT NULL
            );
            ",
        )?;
        Ok(())
    }

    /// Insert a new event.
    pub async fn insert_event(&self, event: &Event) -> anyhow::Result<i64> {
        let conn = self.conn.lock().await;
        let data_json = event
            .data
            .as_ref()
            .map(|d| serde_json::to_string(d).unwrap_or_default());

        conn.execute(
            "INSERT INTO events (timestamp, event_type, severity, message, data) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                event.timestamp.to_rfc3339(),
                event.event_type,
                event.severity.to_string(),
                event.message,
                data_json,
            ],
        )?;
        Ok(conn.last_insert_rowid())
    }

    /// Query the most recent events, up to `limit`.
    pub async fn query_recent_events(&self, limit: u32) -> anyhow::Result<Vec<Event>> {
        let conn = self.conn.lock().await;
        let mut stmt = conn.prepare(
            "SELECT id, timestamp, event_type, severity, message, data FROM events ORDER BY timestamp DESC LIMIT ?1",
        )?;

        let rows = stmt.query_map(params![limit], |row| {
            let id: i64 = row.get(0)?;
            let ts_str: String = row.get(1)?;
            let event_type: String = row.get(2)?;
            let severity_str: String = row.get(3)?;
            let message: String = row.get(4)?;
            let data_str: Option<String> = row.get(5)?;

            Ok(Event {
                id: Some(id),
                timestamp: DateTime::parse_from_rfc3339(&ts_str)
                    .map(|dt| dt.with_timezone(&Utc))
                    .unwrap_or_else(|_| Utc::now()),
                event_type,
                severity: Severity::from_str_lossy(&severity_str),
                message,
                data: data_str.and_then(|s| serde_json::from_str(&s).ok()),
            })
        })?;

        let mut events = Vec::new();
        for row in rows {
            events.push(row?);
        }
        Ok(events)
    }

    /// Delete events older than `retention_days`.
    pub async fn apply_retention(&self, retention_days: u32) -> anyhow::Result<usize> {
        let conn = self.conn.lock().await;
        let cutoff = Utc::now() - chrono::Duration::days(i64::from(retention_days));
        let deleted = conn.execute(
            "DELETE FROM events WHERE timestamp < ?1",
            params![cutoff.to_rfc3339()],
        )?;
        if deleted > 0 {
            info!("Retention: {deleted} Events älter als {retention_days} Tage gelöscht");
            // VACUUM nach Loeschung um Speicherplatz freizugeben
            let _ = conn.execute_batch("VACUUM;");
        }
        Ok(deleted)
    }

    /// Aggregate monitoring events older than `aggregate_after_days` into 5-minute buckets.
    /// Only aggregates events with event_type starting with "monitoring." and then deletes
    /// the original rows.
    pub async fn aggregate_monitoring_events(
        &self,
        aggregate_after_days: u32,
    ) -> anyhow::Result<usize> {
        let conn = self.conn.lock().await;
        let cutoff = Utc::now() - chrono::Duration::days(i64::from(aggregate_after_days));
        let cutoff_str = cutoff.to_rfc3339();

        // Count how many rows will be aggregated
        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM events WHERE event_type LIKE 'monitoring.%' AND timestamp < ?1",
            params![cutoff_str],
            |row| row.get(0),
        )?;

        if count == 0 {
            return Ok(0);
        }

        // Insert aggregates: group by 5-minute bucket and event_type
        conn.execute(
            "INSERT INTO monitoring_aggregates (bucket_start, event_type, count, avg_value, min_value, max_value)
             SELECT
                 strftime('%Y-%m-%dT%H:', timestamp) || printf('%02d', (CAST(strftime('%M', timestamp) AS INTEGER) / 5) * 5) || ':00Z' as bucket,
                 event_type,
                 COUNT(*),
                 AVG(CAST(json_extract(data, '$.value') AS REAL)),
                 MIN(CAST(json_extract(data, '$.value') AS REAL)),
                 MAX(CAST(json_extract(data, '$.value') AS REAL))
             FROM events
             WHERE event_type LIKE 'monitoring.%' AND timestamp < ?1
             GROUP BY bucket, event_type",
            params![cutoff_str],
        )?;

        // Delete aggregated source rows
        let deleted = conn.execute(
            "DELETE FROM events WHERE event_type LIKE 'monitoring.%' AND timestamp < ?1",
            params![cutoff_str],
        )?;

        if deleted > 0 {
            info!(
                "Aggregation: {deleted} Monitoring-Events zu 5-Min-Buckets zusammengefasst"
            );
            // VACUUM nach Aggregation um Speicherplatz freizugeben
            let _ = conn.execute_batch("VACUUM;");
        }

        Ok(deleted)
    }

    /// Save or update recovery state.
    pub async fn upsert_recovery_state(&self, state: &RecoveryState) -> anyhow::Result<()> {
        let conn = self.conn.lock().await;
        conn.execute(
            "INSERT OR REPLACE INTO recovery_state (id, stage, started_at, updated_at, status)
             VALUES (1, ?1, ?2, ?3, ?4)",
            params![
                state.stage,
                state.started_at.to_rfc3339(),
                state.updated_at.to_rfc3339(),
                state.status,
            ],
        )?;
        Ok(())
    }

    /// Insert a fallback override record.
    pub async fn insert_fallback_override(
        &self,
        override_rec: &FallbackOverride,
    ) -> anyhow::Result<()> {
        let conn = self.conn.lock().await;
        conn.execute(
            "INSERT INTO fallback_overrides (compose_file, service_name, override_path, created_at)
             VALUES (?1, ?2, ?3, ?4)",
            params![
                override_rec.compose_file,
                override_rec.service_name,
                override_rec.override_path,
                override_rec.created_at.to_rfc3339(),
            ],
        )?;
        Ok(())
    }

    /// Load the current recovery state (if any).
    pub async fn load_recovery_state(&self) -> anyhow::Result<Option<RecoveryState>> {
        let conn = self.conn.lock().await;
        let mut stmt = conn.prepare(
            "SELECT id, stage, started_at, updated_at, status FROM recovery_state WHERE id = 1",
        )?;

        let mut rows = stmt.query_map([], |row| {
            let id: i64 = row.get(0)?;
            let stage: String = row.get(1)?;
            let started_at_str: String = row.get(2)?;
            let updated_at_str: String = row.get(3)?;
            let status: String = row.get(4)?;

            Ok(RecoveryState {
                id: Some(id),
                stage,
                started_at: DateTime::parse_from_rfc3339(&started_at_str)
                    .map(|dt| dt.with_timezone(&Utc))
                    .unwrap_or_else(|_| Utc::now()),
                updated_at: DateTime::parse_from_rfc3339(&updated_at_str)
                    .map(|dt| dt.with_timezone(&Utc))
                    .unwrap_or_else(|_| Utc::now()),
                status,
            })
        })?;

        match rows.next() {
            Some(Ok(state)) => Ok(Some(state)),
            Some(Err(e)) => Err(e.into()),
            None => Ok(None),
        }
    }

    /// Clear recovery state (mark as completed).
    pub async fn clear_recovery_state(&self) -> anyhow::Result<()> {
        let conn = self.conn.lock().await;
        conn.execute("DELETE FROM recovery_state WHERE id = 1", [])?;
        Ok(())
    }

    /// Load all fallback overrides.
    pub async fn load_fallback_overrides(&self) -> anyhow::Result<Vec<FallbackOverride>> {
        let conn = self.conn.lock().await;
        let mut stmt = conn.prepare(
            "SELECT id, compose_file, service_name, override_path, created_at FROM fallback_overrides",
        )?;

        let rows = stmt.query_map([], |row| {
            let id: i64 = row.get(0)?;
            let compose_file: String = row.get(1)?;
            let service_name: String = row.get(2)?;
            let override_path: String = row.get(3)?;
            let created_at_str: String = row.get(4)?;

            Ok(FallbackOverride {
                id: Some(id),
                compose_file,
                service_name,
                override_path,
                created_at: DateTime::parse_from_rfc3339(&created_at_str)
                    .map(|dt| dt.with_timezone(&Utc))
                    .unwrap_or_else(|_| Utc::now()),
            })
        })?;

        let mut overrides = Vec::new();
        for row in rows {
            overrides.push(row?);
        }
        Ok(overrides)
    }

    // ─── Lease-Persistenz ─────────────────────────────────────────────────
    // Leases werden in SQLite gespeichert um Daemon-Neustarts zu überleben.
    // Bei Restart werden nicht-abgelaufene Leases wiederhergestellt.

    /// Speichert einen aktiven Lease in der Datenbank.
    pub async fn save_lease(
        &self,
        lease_id: &str,
        pipeline: &str,
        gpu_device: &str,
        workload_type: &str,
        vram_mb: u64,
        acquired_at: &DateTime<Utc>,
        expires_at: &DateTime<Utc>,
    ) -> anyhow::Result<()> {
        let conn = self.conn.lock().await;
        conn.execute(
            "INSERT OR REPLACE INTO active_leases
             (lease_id, pipeline, gpu_device, workload_type, vram_mb, acquired_at, expires_at, last_heartbeat)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                lease_id,
                pipeline,
                gpu_device,
                workload_type,
                vram_mb as i64,
                acquired_at.to_rfc3339(),
                expires_at.to_rfc3339(),
                Utc::now().to_rfc3339(),
            ],
        )?;
        Ok(())
    }

    /// Entfernt einen Lease aus der Datenbank (nach Release).
    pub async fn remove_lease(&self, lease_id: &str) -> anyhow::Result<()> {
        let conn = self.conn.lock().await;
        conn.execute(
            "DELETE FROM active_leases WHERE lease_id = ?1",
            params![lease_id],
        )?;
        Ok(())
    }

    /// Aktualisiert den Heartbeat-Timestamp eines Leases.
    pub async fn update_lease_heartbeat(&self, lease_id: &str) -> anyhow::Result<()> {
        let conn = self.conn.lock().await;
        conn.execute(
            "UPDATE active_leases SET last_heartbeat = ?1 WHERE lease_id = ?2",
            params![Utc::now().to_rfc3339(), lease_id],
        )?;
        Ok(())
    }

    /// Lädt alle nicht-abgelaufenen Leases (für Daemon-Restart-Recovery).
    pub async fn load_active_leases(&self) -> anyhow::Result<Vec<PersistedLease>> {
        let conn = self.conn.lock().await;
        let now = Utc::now().to_rfc3339();

        let mut stmt = conn.prepare(
            "SELECT lease_id, pipeline, gpu_device, workload_type, vram_mb, acquired_at, expires_at, last_heartbeat
             FROM active_leases WHERE expires_at > ?1",
        )?;

        let rows = stmt.query_map(params![now], |row| {
            Ok(PersistedLease {
                lease_id: row.get(0)?,
                pipeline: row.get(1)?,
                gpu_device: row.get(2)?,
                workload_type: row.get(3)?,
                vram_mb: row.get::<_, i64>(4)? as u64,
                acquired_at: row.get(5)?,
                expires_at: row.get(6)?,
                last_heartbeat: row.get(7)?,
            })
        })?;

        let mut leases = Vec::new();
        for row in rows {
            leases.push(row?);
        }
        Ok(leases)
    }

    /// Entfernt alle abgelaufenen Leases.
    pub async fn clean_expired_leases(&self) -> anyhow::Result<usize> {
        let conn = self.conn.lock().await;
        let now = Utc::now().to_rfc3339();
        let deleted = conn.execute(
            "DELETE FROM active_leases WHERE expires_at <= ?1",
            params![now],
        )?;
        if deleted > 0 {
            info!("Lease-Cleanup: {} abgelaufene Leases entfernt", deleted);
        }
        Ok(deleted)
    }

    /// Datenbankgroesse in MB pruefen (fuer max_db_size_mb Monitoring).
    pub fn check_db_size_mb(&self) -> Option<u64> {
        self.db_path.as_ref().and_then(|path| {
            std::fs::metadata(path)
                .ok()
                .map(|m| m.len() / (1024 * 1024))
        })
    }

    /// GPU-Telemetrie in die Datenbank loggen (einmal pro Intervall).
    pub async fn log_gpu_telemetry(
        &self,
        pci_address: &str,
        gpu_type: &str,
        temperature_c: u32,
        utilization_pct: u32,
        memory_used_mb: u64,
        memory_total_mb: u64,
        power_draw_w: f64,
        pstate: &str,
        fan_speed_pct: u32,
        clock_graphics_mhz: u32,
        health_score: Option<f64>,
        warning_level: Option<&str>,
    ) -> anyhow::Result<()> {
        let conn = self.conn.lock().await;
        let now = Utc::now().to_rfc3339();
        conn.execute(
            "INSERT INTO gpu_telemetry (timestamp, pci_address, gpu_type, temperature_c,
             utilization_pct, memory_used_mb, memory_total_mb, power_draw_w, pstate,
             fan_speed_pct, clock_graphics_mhz, health_score, warning_level)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
            params![
                now,
                pci_address,
                gpu_type,
                temperature_c,
                utilization_pct,
                memory_used_mb as i64,
                memory_total_mb as i64,
                power_draw_w,
                pstate,
                fan_speed_pct,
                clock_graphics_mhz,
                health_score,
                warning_level,
            ],
        )?;
        Ok(())
    }

    /// GPU-Telemetrie der letzten N Stunden abfragen (fuer Auswertung).
    pub async fn query_telemetry(
        &self,
        pci_address: &str,
        hours: u32,
    ) -> anyhow::Result<Vec<serde_json::Value>> {
        let conn = self.conn.lock().await;
        let since = (Utc::now() - chrono::Duration::hours(i64::from(hours))).to_rfc3339();
        let mut stmt = conn.prepare(
            "SELECT timestamp, temperature_c, utilization_pct, memory_used_mb,
                    power_draw_w, pstate, fan_speed_pct, clock_graphics_mhz,
                    health_score, warning_level
             FROM gpu_telemetry
             WHERE pci_address = ?1 AND timestamp >= ?2
             ORDER BY timestamp ASC",
        )?;

        let rows = stmt
            .query_map(params![pci_address, since], |row| {
                Ok(serde_json::json!({
                    "timestamp": row.get::<_, String>(0)?,
                    "temperature_c": row.get::<_, i32>(1)?,
                    "utilization_pct": row.get::<_, i32>(2)?,
                    "memory_used_mb": row.get::<_, i64>(3)?,
                    "power_draw_w": row.get::<_, f64>(4)?,
                    "pstate": row.get::<_, String>(5)?,
                    "fan_speed_pct": row.get::<_, i32>(6)?,
                    "clock_graphics_mhz": row.get::<_, i32>(7)?,
                    "health_score": row.get::<_, Option<f64>>(8)?,
                    "warning_level": row.get::<_, Option<String>>(9)?,
                }))
            })?
            .filter_map(|r| r.ok())
            .collect();

        Ok(rows)
    }

    /// Alte Telemetrie-Daten aufräumen (älter als retention_days).
    pub async fn clean_telemetry(&self, retention_days: u32) -> anyhow::Result<usize> {
        let conn = self.conn.lock().await;
        let cutoff = (Utc::now() - chrono::Duration::days(i64::from(retention_days))).to_rfc3339();
        let deleted = conn.execute(
            "DELETE FROM gpu_telemetry WHERE timestamp < ?1",
            params![cutoff],
        )?;
        Ok(deleted)
    }

    /// Helper: insert event shorthand
    pub async fn log_event(
        &self,
        event_type: &str,
        severity: Severity,
        message: &str,
        data: Option<serde_json::Value>,
    ) -> anyhow::Result<()> {
        let event = Event {
            id: None,
            timestamp: Utc::now(),
            event_type: event_type.to_string(),
            severity,
            message: message.to_string(),
            data,
        };
        self.insert_event(&event).await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_insert_and_query_events() {
        let db = EventDb::open_in_memory().unwrap();

        db.log_event("test.event", Severity::Info, "Test message", None)
            .await
            .unwrap();
        db.log_event(
            "test.event2",
            Severity::Warning,
            "Warning message",
            Some(serde_json::json!({"key": "value"})),
        )
        .await
        .unwrap();

        let events = db.query_recent_events(10).await.unwrap();
        assert_eq!(events.len(), 2);
        // Most recent first
        assert_eq!(events[0].event_type, "test.event2");
        assert_eq!(events[0].severity, Severity::Warning);
        assert!(events[0].data.is_some());
        assert_eq!(events[1].event_type, "test.event");
    }

    #[tokio::test]
    async fn test_retention() {
        let db = EventDb::open_in_memory().unwrap();

        // Insert an event with old timestamp
        let old_event = Event {
            id: None,
            timestamp: Utc::now() - chrono::Duration::days(100),
            event_type: "old.event".to_string(),
            severity: Severity::Info,
            message: "Old".to_string(),
            data: None,
        };
        db.insert_event(&old_event).await.unwrap();

        // Insert a recent event
        db.log_event("new.event", Severity::Info, "New", None)
            .await
            .unwrap();

        let deleted = db.apply_retention(90).await.unwrap();
        assert_eq!(deleted, 1);

        let events = db.query_recent_events(10).await.unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event_type, "new.event");
    }

    #[tokio::test]
    async fn test_aggregate_monitoring() {
        let db = EventDb::open_in_memory().unwrap();

        // Insert monitoring events with old timestamps
        for i in 0..5 {
            let event = Event {
                id: None,
                timestamp: Utc::now() - chrono::Duration::days(10),
                event_type: "monitoring.aer".to_string(),
                severity: Severity::Info,
                message: format!("AER count {i}"),
                data: Some(serde_json::json!({"value": i})),
            };
            db.insert_event(&event).await.unwrap();
        }

        // Insert a non-monitoring event (should not be aggregated)
        let event = Event {
            id: None,
            timestamp: Utc::now() - chrono::Duration::days(10),
            event_type: "warning.level_change".to_string(),
            severity: Severity::Warning,
            message: "Level changed".to_string(),
            data: None,
        };
        db.insert_event(&event).await.unwrap();

        let aggregated = db.aggregate_monitoring_events(7).await.unwrap();
        assert_eq!(aggregated, 5);

        // The non-monitoring event should still exist
        let remaining = db.query_recent_events(10).await.unwrap();
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].event_type, "warning.level_change");
    }

    #[tokio::test]
    async fn test_recovery_state() {
        let db = EventDb::open_in_memory().unwrap();

        let state = RecoveryState {
            id: None,
            stage: "flr_reset".to_string(),
            started_at: Utc::now(),
            updated_at: Utc::now(),
            status: "in_progress".to_string(),
        };
        db.upsert_recovery_state(&state).await.unwrap();

        // Update
        let state2 = RecoveryState {
            id: None,
            stage: "flr_reset".to_string(),
            started_at: Utc::now(),
            updated_at: Utc::now(),
            status: "completed".to_string(),
        };
        db.upsert_recovery_state(&state2).await.unwrap();
    }

    #[tokio::test]
    async fn test_lease_persistence_save_and_load() {
        let db = EventDb::open_in_memory().unwrap();
        let now = Utc::now();
        let expires = now + chrono::Duration::hours(2);

        db.save_lease(
            "lease-abc123",
            "audit_designer",
            "0000:05:00.0",
            "embeddings",
            4000,
            &now,
            &expires,
        )
        .await
        .unwrap();

        let leases = db.load_active_leases().await.unwrap();
        assert_eq!(leases.len(), 1);
        assert_eq!(leases[0].lease_id, "lease-abc123");
        assert_eq!(leases[0].pipeline, "audit_designer");
        assert_eq!(leases[0].vram_mb, 4000);
    }

    #[tokio::test]
    async fn test_lease_persistence_remove() {
        let db = EventDb::open_in_memory().unwrap();
        let now = Utc::now();
        let expires = now + chrono::Duration::hours(2);

        db.save_lease("lease-1", "app1", "0000:05:00.0", "llm", 2000, &now, &expires)
            .await
            .unwrap();

        db.remove_lease("lease-1").await.unwrap();

        let leases = db.load_active_leases().await.unwrap();
        assert_eq!(leases.len(), 0);
    }

    #[tokio::test]
    async fn test_lease_persistence_expired_not_loaded() {
        let db = EventDb::open_in_memory().unwrap();
        let now = Utc::now();
        let expired = now - chrono::Duration::hours(1); // Bereits abgelaufen

        db.save_lease(
            "lease-expired",
            "app1",
            "0000:05:00.0",
            "llm",
            2000,
            &(now - chrono::Duration::hours(3)),
            &expired,
        )
        .await
        .unwrap();

        let leases = db.load_active_leases().await.unwrap();
        assert_eq!(leases.len(), 0); // Abgelaufen → nicht geladen
    }

    #[tokio::test]
    async fn test_lease_persistence_clean_expired() {
        let db = EventDb::open_in_memory().unwrap();
        let now = Utc::now();

        // Abgelaufener Lease
        db.save_lease(
            "lease-old",
            "app1",
            "0000:05:00.0",
            "llm",
            2000,
            &(now - chrono::Duration::hours(5)),
            &(now - chrono::Duration::hours(1)),
        )
        .await
        .unwrap();

        // Aktiver Lease
        db.save_lease(
            "lease-new",
            "app2",
            "0000:02:00.0",
            "embeddings",
            4000,
            &now,
            &(now + chrono::Duration::hours(2)),
        )
        .await
        .unwrap();

        let cleaned = db.clean_expired_leases().await.unwrap();
        assert_eq!(cleaned, 1);

        // Nur der aktive Lease bleibt
        let leases = db.load_active_leases().await.unwrap();
        assert_eq!(leases.len(), 1);
        assert_eq!(leases[0].lease_id, "lease-new");
    }

    #[tokio::test]
    async fn test_lease_heartbeat_update() {
        let db = EventDb::open_in_memory().unwrap();
        let now = Utc::now();
        let expires = now + chrono::Duration::hours(2);

        db.save_lease("lease-hb", "app1", "0000:05:00.0", "llm", 2000, &now, &expires)
            .await
            .unwrap();

        db.update_lease_heartbeat("lease-hb").await.unwrap();

        let leases = db.load_active_leases().await.unwrap();
        assert_eq!(leases.len(), 1);
        // Heartbeat sollte aktualisiert sein
        assert_ne!(leases[0].last_heartbeat, leases[0].acquired_at);
    }

    #[tokio::test]
    async fn test_fallback_override() {
        let db = EventDb::open_in_memory().unwrap();

        let override_rec = FallbackOverride {
            id: None,
            compose_file: "/path/to/compose.yml".to_string(),
            service_name: "worker".to_string(),
            override_path: "/path/to/override.yml".to_string(),
            created_at: Utc::now(),
        };
        db.insert_fallback_override(&override_rec).await.unwrap();
    }
}
