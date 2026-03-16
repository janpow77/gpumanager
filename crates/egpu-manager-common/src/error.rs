use thiserror::Error;

#[derive(Error, Debug)]
pub enum EgpuError {
    #[error("GPU-Fehler: {0}")]
    Gpu(#[from] GpuError),

    #[error("Konfigurations-Fehler: {0}")]
    Config(String),

    #[error("IO-Fehler: {0}")]
    Io(#[from] std::io::Error),

    #[error("Sysfs nicht lesbar: {path}")]
    SysfsReadError { path: String },
}

#[derive(Error, Debug)]
pub enum GpuError {
    #[error("nvidia-smi Timeout nach {timeout_secs}s")]
    NvidiaSmiTimeout { timeout_secs: u64 },

    #[error("nvidia-smi Parse-Fehler: {0}")]
    NvidiaSmiParse(String),

    #[error("nvidia-smi nicht verfügbar: {0}")]
    NvidiaSmiUnavailable(String),

    #[error("GPU {pci_address} nicht gefunden")]
    GpuNotFound { pci_address: String },
}

#[derive(Error, Debug)]
pub enum AerError {
    #[error("AER-Zähler nicht lesbar: {0}")]
    ReadError(String),
}

#[derive(Error, Debug)]
pub enum PcieError {
    #[error("PCIe-Link nicht lesbar für {pci_address}: {reason}")]
    LinkReadError { pci_address: String, reason: String },

    #[error("PCIe-Reset fehlgeschlagen für {pci_address}: {reason}")]
    ResetFailed { pci_address: String, reason: String },
}

#[derive(Error, Debug)]
pub enum ThunderboltError {
    #[error("Thunderbolt-Gerät {device_path} nicht verfügbar: {reason}")]
    DeviceError { device_path: String, reason: String },
}

#[derive(Error, Debug)]
pub enum DockerError {
    #[error("Docker nicht erreichbar: {0}")]
    Unreachable(String),

    #[error("Container {name} nicht gefunden")]
    ContainerNotFound { name: String },

    #[error("Container-Operation fehlgeschlagen: {0}")]
    OperationFailed(String),

    #[error("Docker-API Timeout")]
    Timeout,
}

#[derive(Error, Debug)]
pub enum OllamaError {
    #[error("Ollama nicht erreichbar: {0}")]
    Unreachable(String),

    #[error("Ollama-API Fehler: {0}")]
    ApiError(String),
}

#[derive(Error, Debug)]
pub enum WatchdogError {
    #[error("Watchdog-Binary nicht gefunden: {0}")]
    BinaryNotFound(String),

    #[error("Watchdog-Start fehlgeschlagen: {0}")]
    StartFailed(String),
}
