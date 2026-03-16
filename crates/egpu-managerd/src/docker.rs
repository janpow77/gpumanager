use std::collections::HashMap;
use std::path::Path;
use std::time::Duration;

use async_trait::async_trait;
use egpu_manager_common::error::DockerError;
use egpu_manager_common::hal::{ContainerInfo, DockerControl};
use tracing::{debug, info, warn};

use crate::recovery::{generate_override_path, generate_override_yaml};

/// Maximum number of retry attempts for Docker commands.
const MAX_RETRIES: u32 = 3;

/// Delay between retries in milliseconds.
const RETRY_DELAY_MS: u64 = 1000;

/// Real Docker control implementation using `docker compose` CLI.
pub struct DockerComposeControl {
    /// Timeout for docker commands.
    command_timeout: Duration,
    /// Timeout for container stop operations.
    stop_timeout: Duration,
}

impl DockerComposeControl {
    pub fn new(command_timeout_secs: u64, stop_timeout_secs: u64) -> Self {
        Self {
            command_timeout: Duration::from_secs(command_timeout_secs),
            stop_timeout: Duration::from_secs(stop_timeout_secs),
        }
    }

    /// Run a command with retry logic.
    async fn run_with_retry(
        &self,
        program: &str,
        args: &[&str],
        timeout: Duration,
    ) -> Result<String, DockerError> {
        let mut last_err = DockerError::Timeout;

        for attempt in 1..=MAX_RETRIES {
            match self.run_command(program, args, timeout).await {
                Ok(output) => return Ok(output),
                Err(e) => {
                    last_err = e;
                    if attempt < MAX_RETRIES {
                        warn!(
                            "Docker-Befehl fehlgeschlagen (Versuch {}/{}): {}",
                            attempt, MAX_RETRIES, last_err
                        );
                        tokio::time::sleep(Duration::from_millis(RETRY_DELAY_MS)).await;
                    }
                }
            }
        }

        Err(last_err)
    }

    /// Run a single command with timeout.
    async fn run_command(
        &self,
        program: &str,
        args: &[&str],
        timeout: Duration,
    ) -> Result<String, DockerError> {
        use tokio::process::Command;

        debug!("Docker-Befehl: {} {}", program, args.join(" "));

        let result = tokio::time::timeout(timeout, async {
            Command::new(program)
                .args(args)
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .output()
                .await
        })
        .await;

        match result {
            Ok(Ok(output)) => {
                let stdout = String::from_utf8_lossy(&output.stdout).to_string();
                let stderr = String::from_utf8_lossy(&output.stderr).to_string();

                if output.status.success() {
                    Ok(stdout)
                } else {
                    Err(DockerError::OperationFailed(format!(
                        "Exit-Code {}: {}",
                        output.status,
                        if stderr.is_empty() { &stdout } else { &stderr }
                    )))
                }
            }
            Ok(Err(e)) => Err(DockerError::Unreachable(format!(
                "Befehl nicht ausführbar: {e}"
            ))),
            Err(_) => Err(DockerError::Timeout),
        }
    }
}

#[async_trait]
impl DockerControl for DockerComposeControl {
    async fn recreate_with_env(
        &self,
        compose_file: &str,
        service: &str,
        env: HashMap<String, String>,
    ) -> Result<(), DockerError> {
        // Generate override YAML file
        let override_path = generate_override_path(compose_file, service);

        // Build fallback device from env
        let fallback_device = env
            .get("NVIDIA_VISIBLE_DEVICES")
            .cloned()
            .unwrap_or_default();

        let yaml_content = generate_override_yaml(service, &fallback_device);

        // Write override file
        if let Err(e) = tokio::fs::write(&override_path, &yaml_content).await {
            return Err(DockerError::OperationFailed(format!(
                "Override-Datei nicht schreibbar: {override_path}: {e}"
            )));
        }
        info!("Override-Datei geschrieben: {}", override_path);

        // docker compose -f original -f override up -d --force-recreate service
        let args = vec![
            "compose",
            "-f",
            compose_file,
            "-f",
            &override_path,
            "up",
            "-d",
            "--force-recreate",
            service,
        ];

        self.run_with_retry("docker", &args, self.command_timeout)
            .await?;

        info!(
            "Container {} neu erstellt mit Fallback-GPU (Override: {})",
            service, override_path
        );

        Ok(())
    }

    async fn exec_in_container(
        &self,
        name: &str,
        cmd: &[&str],
        timeout: Duration,
    ) -> Result<String, DockerError> {
        let mut args = vec!["exec", name];
        args.extend(cmd);

        self.run_with_retry("docker", &args, timeout).await
    }

    async fn stop_container(&self, name: &str, timeout: Duration) -> Result<(), DockerError> {
        let timeout_secs = timeout.as_secs().to_string();
        let args = vec!["stop", "--time", &timeout_secs, name];

        self.run_with_retry("docker", &args, self.stop_timeout)
            .await?;

        info!("Container {} gestoppt", name);
        Ok(())
    }

    async fn list_containers(&self) -> Result<Vec<ContainerInfo>, DockerError> {
        let output = self
            .run_with_retry(
                "docker",
                &["ps", "--format", "{{.Names}}\t{{.Status}}\t{{.State}}"],
                self.command_timeout,
            )
            .await?;

        let mut containers = Vec::new();
        for line in output.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            let parts: Vec<&str> = line.split('\t').collect();
            if parts.len() >= 3 {
                containers.push(ContainerInfo {
                    name: parts[0].to_string(),
                    status: parts[1].to_string(),
                    running: parts[2] == "running",
                });
            }
        }

        Ok(containers)
    }
}

/// Check if fallback override files exist on disk.
pub async fn check_existing_overrides(
    overrides: &[crate::db::FallbackOverride],
) -> Vec<crate::db::FallbackOverride> {
    let mut existing = Vec::new();
    for ov in overrides {
        if Path::new(&ov.override_path).exists() {
            info!(
                "Fallback-Override vorhanden: {} (Service: {})",
                ov.override_path, ov.service_name
            );
            existing.push(ov.clone());
        }
    }
    existing
}

/// Mock Docker control for testing.
#[cfg(any(test, feature = "mock-hardware"))]
pub struct MockDockerControl {
    /// Track calls for verification in tests.
    pub exec_calls: std::sync::Arc<tokio::sync::Mutex<Vec<(String, Vec<String>)>>>,
    pub recreate_calls:
        std::sync::Arc<tokio::sync::Mutex<Vec<(String, String, HashMap<String, String>)>>>,
    pub stop_calls: std::sync::Arc<tokio::sync::Mutex<Vec<String>>>,
    /// If set, exec_in_container returns this error.
    pub exec_error: std::sync::Arc<tokio::sync::Mutex<Option<DockerError>>>,
}

#[cfg(any(test, feature = "mock-hardware"))]
impl MockDockerControl {
    pub fn new() -> Self {
        Self {
            exec_calls: std::sync::Arc::new(tokio::sync::Mutex::new(Vec::new())),
            recreate_calls: std::sync::Arc::new(tokio::sync::Mutex::new(Vec::new())),
            stop_calls: std::sync::Arc::new(tokio::sync::Mutex::new(Vec::new())),
            exec_error: std::sync::Arc::new(tokio::sync::Mutex::new(None)),
        }
    }
}

#[cfg(any(test, feature = "mock-hardware"))]
#[async_trait]
impl DockerControl for MockDockerControl {
    async fn recreate_with_env(
        &self,
        compose_file: &str,
        service: &str,
        env: HashMap<String, String>,
    ) -> Result<(), DockerError> {
        let mut calls = self.recreate_calls.lock().await;
        calls.push((compose_file.to_string(), service.to_string(), env));
        Ok(())
    }

    async fn exec_in_container(
        &self,
        name: &str,
        cmd: &[&str],
        _timeout: Duration,
    ) -> Result<String, DockerError> {
        let err = self.exec_error.lock().await;
        if let Some(ref e) = *err {
            return Err(DockerError::OperationFailed(e.to_string()));
        }

        let mut calls = self.exec_calls.lock().await;
        calls.push((
            name.to_string(),
            cmd.iter().map(|s| s.to_string()).collect(),
        ));
        Ok("OK".to_string())
    }

    async fn stop_container(&self, name: &str, _timeout: Duration) -> Result<(), DockerError> {
        let mut calls = self.stop_calls.lock().await;
        calls.push(name.to_string());
        Ok(())
    }

    async fn list_containers(&self) -> Result<Vec<ContainerInfo>, DockerError> {
        Ok(vec![ContainerInfo {
            name: "test-container".to_string(),
            status: "Up 2 hours".to_string(),
            running: true,
        }])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_mock_docker_exec() {
        let docker = MockDockerControl::new();
        let result = docker
            .exec_in_container("test", &["echo", "hello"], Duration::from_secs(5))
            .await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "OK");

        let calls = docker.exec_calls.lock().await;
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, "test");
        assert_eq!(calls[0].1, vec!["echo", "hello"]);
    }

    #[tokio::test]
    async fn test_mock_docker_recreate() {
        let docker = MockDockerControl::new();
        let mut env = HashMap::new();
        env.insert(
            "NVIDIA_VISIBLE_DEVICES".to_string(),
            "0000:02:00.0".to_string(),
        );

        let result = docker
            .recreate_with_env("/path/compose.yml", "worker", env.clone())
            .await;
        assert!(result.is_ok());

        let calls = docker.recreate_calls.lock().await;
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, "/path/compose.yml");
        assert_eq!(calls[0].1, "worker");
    }

    #[tokio::test]
    async fn test_mock_docker_stop() {
        let docker = MockDockerControl::new();
        let result = docker
            .stop_container("test", Duration::from_secs(10))
            .await;
        assert!(result.is_ok());

        let calls = docker.stop_calls.lock().await;
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0], "test");
    }

    #[tokio::test]
    async fn test_mock_docker_list() {
        let docker = MockDockerControl::new();
        let containers = docker.list_containers().await.unwrap();
        assert_eq!(containers.len(), 1);
        assert!(containers[0].running);
    }

    #[tokio::test]
    async fn test_mock_docker_exec_error() {
        let docker = MockDockerControl::new();
        {
            let mut err = docker.exec_error.lock().await;
            *err = Some(DockerError::Timeout);
        }

        let result = docker
            .exec_in_container("test", &["cmd"], Duration::from_secs(5))
            .await;
        assert!(result.is_err());
    }
}
