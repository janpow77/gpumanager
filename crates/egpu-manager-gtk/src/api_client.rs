use std::time::Duration;

use anyhow::Result;
use tracing::debug;

use crate::state::{ConnectionState, PipelineInfo, StatusResponse, WidgetState};

const DAEMON_URL: &str = "http://127.0.0.1:7842";
const POLL_INTERVAL: Duration = Duration::from_secs(5);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(3);

/// Runs the polling loop, sending state updates through the channel.
pub async fn poll_loop(tx: async_channel::Sender<WidgetState>) {
    let client = reqwest::Client::builder()
        .timeout(REQUEST_TIMEOUT)
        .build()
        .expect("HTTP client");

    let mut consecutive_failures: u32 = 0;

    loop {
        let state = fetch_state(&client).await;

        match state {
            Ok(mut ws) => {
                consecutive_failures = 0;
                ws.connection = ConnectionState::Connected;
                if tx.send(ws).await.is_err() {
                    break; // receiver dropped
                }
            }
            Err(e) => {
                consecutive_failures += 1;
                debug!("Daemon-Abfrage fehlgeschlagen ({consecutive_failures}): {e}");

                let conn = if consecutive_failures >= 5 {
                    ConnectionState::Error("Daemon nicht erreichbar".into())
                } else {
                    ConnectionState::Reconnecting(consecutive_failures)
                };

                let ws = WidgetState {
                    connection: conn,
                    ..Default::default()
                };
                if tx.send(ws).await.is_err() {
                    break;
                }
            }
        }

        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

async fn fetch_state(client: &reqwest::Client) -> Result<WidgetState> {
    let (status_res, pipelines_res) = tokio::join!(
        client.get(format!("{DAEMON_URL}/api/status")).send(),
        client.get(format!("{DAEMON_URL}/api/pipelines")).send(),
    );

    let status: StatusResponse = status_res?.json().await?;
    let pipelines: Vec<PipelineInfo> = pipelines_res?.json().await?;

    Ok(WidgetState {
        connection: ConnectionState::Connected,
        daemon: status.daemon,
        health_score: status.health_score,
        gpus: status.gpus.unwrap_or_default(),
        remote_gpus: status.remote_gpus.unwrap_or_default(),
        pipelines,
    })
}
