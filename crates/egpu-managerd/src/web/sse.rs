use std::convert::Infallible;
use std::time::Duration;

use axum::response::sse::{Event as SseEvent, KeepAlive, Sse};
use futures::stream::Stream;
use serde::Serialize;
use tokio::sync::broadcast;
use tokio_stream::wrappers::BroadcastStream;
use tokio_stream::StreamExt;

/// Event types that can be broadcast via SSE.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", content = "data")]
pub enum BroadcastEvent {
    #[serde(rename = "gpu_status")]
    GpuStatus(serde_json::Value),
    #[serde(rename = "warning_level")]
    WarningLevel(serde_json::Value),
    #[serde(rename = "recovery_stage")]
    RecoveryStage(serde_json::Value),
    #[serde(rename = "pipeline_change")]
    PipelineChange(serde_json::Value),
    #[serde(rename = "config_reload")]
    ConfigReload(serde_json::Value),
    #[serde(rename = "health_score")]
    HealthScore(serde_json::Value),
    /// eGPU Safe-Disconnect Warnung/Status fuer Widget/UI
    #[serde(rename = "egpu_disconnect")]
    EgpuDisconnect(serde_json::Value),
}

impl BroadcastEvent {
    fn event_type(&self) -> &'static str {
        match self {
            BroadcastEvent::GpuStatus(_) => "gpu_status",
            BroadcastEvent::WarningLevel(_) => "warning_level",
            BroadcastEvent::RecoveryStage(_) => "recovery_stage",
            BroadcastEvent::PipelineChange(_) => "pipeline_change",
            BroadcastEvent::ConfigReload(_) => "config_reload",
            BroadcastEvent::HealthScore(_) => "health_score",
            BroadcastEvent::EgpuDisconnect(_) => "egpu_disconnect",
        }
    }
}

/// The SSE broadcaster. Wraps a tokio broadcast channel.
#[derive(Clone)]
pub struct SseBroadcaster {
    tx: broadcast::Sender<BroadcastEvent>,
}

impl SseBroadcaster {
    pub fn new(capacity: usize) -> Self {
        let (tx, _) = broadcast::channel(capacity);
        Self { tx }
    }

    /// Send an event to all connected clients.
    /// Returns the number of receivers that got the message.
    pub fn send(&self, event: BroadcastEvent) -> usize {
        self.tx.send(event).unwrap_or(0)
    }

    /// Create an SSE stream for a new client connection.
    pub fn subscribe(
        &self,
    ) -> Sse<impl Stream<Item = Result<SseEvent, Infallible>> + use<>> {
        let rx = self.tx.subscribe();
        let stream = BroadcastStream::new(rx).filter_map(|result: Result<BroadcastEvent, _>| match result {
            Ok(event) => {
                let event_type = event.event_type().to_string();
                match serde_json::to_string(&event) {
                    Ok(json) => Some(Ok(SseEvent::default()
                        .event(event_type)
                        .data(json))),
                    Err(_) => None,
                }
            }
            Err(_) => None,
        });

        Sse::new(stream).keep_alive(
            KeepAlive::new()
                .interval(Duration::from_secs(15))
                .text("heartbeat"),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_broadcaster_creation() {
        let broadcaster = SseBroadcaster::new(64);
        // No subscribers, send returns 0
        let count = broadcaster.send(BroadcastEvent::GpuStatus(
            serde_json::json!({"test": true}),
        ));
        assert_eq!(count, 0);
    }

    #[test]
    fn test_broadcast_event_serialization() {
        let event = BroadcastEvent::WarningLevel(serde_json::json!({"level": "yellow"}));
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("warning_level"));
        assert!(json.contains("yellow"));
    }

    #[test]
    fn test_event_type_names() {
        assert_eq!(
            BroadcastEvent::GpuStatus(serde_json::Value::Null).event_type(),
            "gpu_status"
        );
        assert_eq!(
            BroadcastEvent::WarningLevel(serde_json::Value::Null).event_type(),
            "warning_level"
        );
        assert_eq!(
            BroadcastEvent::RecoveryStage(serde_json::Value::Null).event_type(),
            "recovery_stage"
        );
        assert_eq!(
            BroadcastEvent::PipelineChange(serde_json::Value::Null).event_type(),
            "pipeline_change"
        );
        assert_eq!(
            BroadcastEvent::ConfigReload(serde_json::Value::Null).event_type(),
            "config_reload"
        );
        assert_eq!(
            BroadcastEvent::HealthScore(serde_json::Value::Null).event_type(),
            "health_score"
        );
        assert_eq!(
            BroadcastEvent::EgpuDisconnect(serde_json::Value::Null).event_type(),
            "egpu_disconnect"
        );
    }
}
