//! Lightweight system info from /proc (no external crate needed).

use serde::Serialize;
use std::fs;

#[derive(Debug, Clone, Serialize, Default)]
pub struct SystemStats {
    pub cpu_percent: f64,
    pub cpu_cores: u32,
    pub cpu_model: String,
    pub ram_total_mb: u64,
    pub ram_used_mb: u64,
    pub ram_free_mb: u64,
    pub ram_available_mb: u64,
    pub swap_total_mb: u64,
    pub swap_used_mb: u64,
    pub load_avg_1m: f64,
    pub load_avg_5m: f64,
    pub load_avg_15m: f64,
    pub uptime_seconds: u64,
}

/// Read CPU times from /proc/stat. Returns (total, idle).
fn read_cpu_times() -> Option<(u64, u64)> {
    let stat = fs::read_to_string("/proc/stat").ok()?;
    let line = stat.lines().next()?;
    if !line.starts_with("cpu ") {
        return None;
    }
    let parts: Vec<u64> = line
        .split_whitespace()
        .skip(1)
        .filter_map(|s| s.parse().ok())
        .collect();
    if parts.len() < 4 {
        return None;
    }
    let total: u64 = parts.iter().sum();
    let idle = parts[3] + parts.get(4).copied().unwrap_or(0); // idle + iowait
    Some((total, idle))
}

/// Measure CPU usage over a short interval.
pub async fn measure_cpu_percent() -> f64 {
    let Some((total1, idle1)) = read_cpu_times() else {
        return 0.0;
    };
    tokio::time::sleep(std::time::Duration::from_millis(250)).await;
    let Some((total2, idle2)) = read_cpu_times() else {
        return 0.0;
    };

    let total_diff = total2.saturating_sub(total1) as f64;
    let idle_diff = idle2.saturating_sub(idle1) as f64;

    if total_diff == 0.0 {
        return 0.0;
    }

    ((total_diff - idle_diff) / total_diff * 100.0).clamp(0.0, 100.0)
}

/// Get full system stats.
pub async fn get_system_stats() -> SystemStats {
    let cpu_percent = measure_cpu_percent().await;

    // CPU cores
    let cpu_cores = std::thread::available_parallelism()
        .map(|p| p.get() as u32)
        .unwrap_or(1);

    // CPU model from /proc/cpuinfo
    let cpu_model = fs::read_to_string("/proc/cpuinfo")
        .ok()
        .and_then(|s| {
            s.lines()
                .find(|l| l.starts_with("model name"))
                .and_then(|l| l.split(':').nth(1))
                .map(|s| s.trim().to_string())
        })
        .unwrap_or_default();

    // Memory from /proc/meminfo
    let (ram_total_mb, ram_free_mb, ram_available_mb, swap_total_mb, swap_free_mb) =
        parse_meminfo();
    let ram_used_mb = ram_total_mb.saturating_sub(ram_available_mb);
    let swap_used_mb = swap_total_mb.saturating_sub(swap_free_mb);

    // Load average
    let (load_1, load_5, load_15) = parse_loadavg();

    // Uptime
    let uptime_seconds = parse_uptime();

    SystemStats {
        cpu_percent,
        cpu_cores,
        cpu_model,
        ram_total_mb,
        ram_used_mb,
        ram_free_mb,
        ram_available_mb,
        swap_total_mb,
        swap_used_mb,
        load_avg_1m: load_1,
        load_avg_5m: load_5,
        load_avg_15m: load_15,
        uptime_seconds,
    }
}

fn parse_meminfo() -> (u64, u64, u64, u64, u64) {
    let content = fs::read_to_string("/proc/meminfo").unwrap_or_default();
    let mut total = 0u64;
    let mut free = 0u64;
    let mut available = 0u64;
    let mut swap_total = 0u64;
    let mut swap_free = 0u64;

    for line in content.lines() {
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() < 2 {
            continue;
        }
        let val_kb: u64 = parts[1].parse().unwrap_or(0);
        let val_mb = val_kb / 1024;
        match parts[0] {
            "MemTotal:" => total = val_mb,
            "MemFree:" => free = val_mb,
            "MemAvailable:" => available = val_mb,
            "SwapTotal:" => swap_total = val_mb,
            "SwapFree:" => swap_free = val_mb,
            _ => {}
        }
    }
    (total, free, available, swap_total, swap_free)
}

fn parse_loadavg() -> (f64, f64, f64) {
    let content = fs::read_to_string("/proc/loadavg").unwrap_or_default();
    let parts: Vec<&str> = content.split_whitespace().collect();
    let l1 = parts.first().and_then(|s| s.parse().ok()).unwrap_or(0.0);
    let l5 = parts.get(1).and_then(|s| s.parse().ok()).unwrap_or(0.0);
    let l15 = parts.get(2).and_then(|s| s.parse().ok()).unwrap_or(0.0);
    (l1, l5, l15)
}

fn parse_uptime() -> u64 {
    let content = fs::read_to_string("/proc/uptime").unwrap_or_default();
    content
        .split_whitespace()
        .next()
        .and_then(|s| s.parse::<f64>().ok())
        .map(|f| f as u64)
        .unwrap_or(0)
}
