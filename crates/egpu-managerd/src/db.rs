use std::path::Path;
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
#[derive(Clone)]
pub struct EventDb {
    conn: Arc<Mutex<Connection>>,
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
        })
    }

    /// Open an in-memory database (for tests).
    #[cfg(test)]
    pub fn open_in_memory() -> anyhow::Result<Self> {
        let conn = Connection::open_in_memory()?;
        Self::create_tables(&conn)?;
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
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

    /// Remove a fallback override by service name.
    pub async fn remove_fallback_override(&self, service_name: &str) -> anyhow::Result<()> {
        let conn = self.conn.lock().await;
        conn.execute(
            "DELETE FROM fallback_overrides WHERE service_name = ?1",
            params![service_name],
        )?;
        Ok(())
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
