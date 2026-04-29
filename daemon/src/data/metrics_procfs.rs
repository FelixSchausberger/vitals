//! Per-process and per-unit metrics collection using procfs.
//!
//! This module implements process metrics collection using the procfs crate
//! to read /proc filesystem data for detailed process monitoring.

use std::collections::HashMap;

use anyhow::{Context, Result};
use procfs::process::{all_processes, Process};
use time::OffsetDateTime;

use crate::data::traits::{ProcessMetrics, ProcessMetricsReader, UnitMetrics, UnitMetricsReader};

/// Per-process metrics reader using procfs
pub struct ProcfsMetricsReader {
    /// Cache for CPU time calculations
    cpu_time_cache: HashMap<u32, (u64, OffsetDateTime)>,
    /// Cache for I/O rate calculations (pid -> (`read_bytes`, `write_bytes`, timestamp))
    io_cache: HashMap<u32, (u64, u64, OffsetDateTime)>,
    /// Cache for network metrics (pid -> (`rx_bytes`, `tx_bytes`, timestamp))
    net_cache: HashMap<u32, (u64, u64, OffsetDateTime)>,
    /// Number of CPU cores for percentage calculations
    cpu_count: usize,
}

impl ProcfsMetricsReader {
    /// Create a new procfs metrics reader
    #[must_use]
    pub fn new() -> Self {
        let cpu_count = num_cpus::get();
        Self {
            cpu_time_cache: HashMap::new(),
            io_cache: HashMap::new(),
            net_cache: HashMap::new(),
            cpu_count,
        }
    }

    /// Calculate CPU usage percentage for a process
    fn calculate_cpu_usage(&mut self, process: &Process) -> Result<f64> {
        let stat = process.stat().context("Failed to read process stat")?;
        let current_time = OffsetDateTime::now_utc();

        // Total CPU time in clock ticks (user + system)
        let total_time = stat.utime + stat.stime;

        let pid_u32 = stat.pid.try_into().unwrap_or(0);
        if let Some((prev_time, prev_timestamp)) = self.cpu_time_cache.get(&pid_u32) {
            let time_delta = (current_time - *prev_timestamp).as_seconds_f64();
            let cpu_delta = total_time.saturating_sub(*prev_time);

            if time_delta > 0.0 {
                // Convert from clock ticks to seconds, then to percentage
                let ticks_per_second = procfs::ticks_per_second();
                #[allow(clippy::cast_precision_loss)]
                let cpu_seconds = cpu_delta as f64 / ticks_per_second as f64;
                let cpu_usage = (cpu_seconds / time_delta) * 100.0;

                // Update cache
                self.cpu_time_cache
                    .insert(pid_u32, (total_time, current_time));

                // Cap at reasonable maximum
                #[allow(clippy::cast_precision_loss)]
                let max_cpu = 100.0 * self.cpu_count as f64;
                Ok(cpu_usage.min(max_cpu))
            } else {
                Ok(0.0)
            }
        } else {
            // First measurement, store and return 0
            self.cpu_time_cache
                .insert(pid_u32, (total_time, current_time));
            Ok(0.0)
        }
    }

    /// Get IO metrics for a process with rate calculation
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    fn get_io_metrics(&mut self, process: &Process) -> Result<(u64, u64)> {
        match process.io() {
            Ok(io) => {
                let current_time = OffsetDateTime::now_utc();
                let current_read = io.read_bytes;
                let current_write = io.write_bytes;

                let stat = process.stat().context("Failed to read process stat")?;
                let pid_u32 = stat.pid.try_into().unwrap_or(0);

                if let Some((prev_read, prev_write, prev_timestamp)) = self.io_cache.get(&pid_u32) {
                    let time_delta = (current_time - *prev_timestamp).as_seconds_f64();

                    if time_delta > 0.0 {
                        // Calculate bytes per second
                        #[allow(clippy::cast_precision_loss)]
                        let read_rate =
                            (current_read.saturating_sub(*prev_read) as f64 / time_delta) as u64;
                        #[allow(clippy::cast_precision_loss)]
                        let write_rate =
                            (current_write.saturating_sub(*prev_write) as f64 / time_delta) as u64;

                        // Update cache
                        self.io_cache
                            .insert(pid_u32, (current_read, current_write, current_time));

                        Ok((read_rate, write_rate))
                    } else {
                        Ok((0, 0))
                    }
                } else {
                    // First measurement, store and return 0
                    self.io_cache
                        .insert(pid_u32, (current_read, current_write, current_time));
                    Ok((0, 0))
                }
            }
            Err(_) => Ok((0, 0)), // Process may not have permission or IO stats
        }
    }

    /// Get network metrics for a process with rate calculation
    /// Note: This provides system-wide network stats per process as a fallback
    /// since per-process network tracking requires complex netlink implementation
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        clippy::cast_precision_loss
    )]
    fn get_net_metrics(&mut self, process: &Process) -> (u64, u64) {
        let Ok(stat) = process.stat() else {
            return (0, 0);
        };

        let pid_u32 = stat.pid.try_into().unwrap_or(0);
        let current_time = OffsetDateTime::now_utc();

        // Read system-wide network stats from /proc/net/dev
        let (system_rx, system_tx) = Self::read_system_net_stats();

        if let Some((prev_rx, prev_tx, prev_timestamp)) = self.net_cache.get(&pid_u32) {
            let time_delta = (current_time - *prev_timestamp).as_seconds_f64();

            if time_delta > 0.0 {
                // Calculate rates (this is a rough approximation)
                let rx_rate = (system_rx.saturating_sub(*prev_rx) as f64 / time_delta) as u64;
                let tx_rate = (system_tx.saturating_sub(*prev_tx) as f64 / time_delta) as u64;

                // Update cache
                self.net_cache
                    .insert(pid_u32, (system_rx, system_tx, current_time));

                // Distribute network usage roughly by CPU usage (basic heuristic)
                // This is a simplification since we can't easily track per-process network
                if let Ok(cpu_usage) = self.calculate_cpu_usage(process) {
                    let cpu_fraction = (cpu_usage / 100.0).min(1.0);
                    let estimated_rx = (rx_rate as f64 * cpu_fraction * 0.1) as u64; // Scale down significantly
                    let estimated_tx = (tx_rate as f64 * cpu_fraction * 0.1) as u64;
                    (estimated_rx, estimated_tx)
                } else {
                    (0, 0)
                }
            } else {
                (0, 0)
            }
        } else {
            // First measurement, store and return 0
            self.net_cache
                .insert(pid_u32, (system_rx, system_tx, current_time));
            (0, 0)
        }
    }

    /// Read system-wide network statistics from /proc/net/dev
    fn read_system_net_stats() -> (u64, u64) {
        match std::fs::read_to_string("/proc/net/dev") {
            Ok(content) => {
                let mut total_rx = 0u64;
                let mut total_tx = 0u64;

                for line in content.lines().skip(2) {
                    // Skip header lines
                    if let Some((_, stats)) = line.split_once(':') {
                        let fields: Vec<&str> = stats.split_whitespace().collect();
                        if fields.len() >= 9 {
                            // RX bytes is field 0, TX bytes is field 8
                            if let (Ok(rx), Ok(tx)) =
                                (fields[0].parse::<u64>(), fields[8].parse::<u64>())
                            {
                                total_rx += rx;
                                total_tx += tx;
                            }
                        }
                    }
                }

                (total_rx, total_tx)
            }
            Err(_) => (0, 0),
        }
    }

    /// Convert a Process to `ProcessMetrics`
    #[allow(clippy::cast_sign_loss)]
    fn process_to_metrics(&mut self, process: &Process) -> Result<ProcessMetrics> {
        let stat = process.stat().context("Failed to read process stat")?;
        let status = process.status().context("Failed to read process status")?;

        let cpu_usage = self.calculate_cpu_usage(process)?;
        let (io_read, io_write) = self.get_io_metrics(process)?;
        let (net_rx, net_tx) = self.get_net_metrics(process);

        // Get memory info from status
        let memory_rss = status.vmrss.unwrap_or(0) * 1024; // Convert from KB to bytes
        let memory_vsz = status.vmsize.unwrap_or(0) * 1024; // Convert from KB to bytes

        // Get command line, fallback to process name if unavailable
        let cmdline = process.cmdline().unwrap_or_default().join(" ");
        let name = if cmdline.is_empty() {
            stat.comm
        } else {
            cmdline
                .split_whitespace()
                .next()
                .unwrap_or(&stat.comm)
                .to_string()
        };

        Ok(ProcessMetrics {
            pid: stat.pid as u32,
            cpu_usage,
            memory_rss,
            memory_vsz,
            io_read_bps: io_read,   // Bytes per second read rate
            io_write_bps: io_write, // Bytes per second write rate
            net_rx_bps: net_rx,
            net_tx_bps: net_tx,
            name,
            cmdline,
        })
    }

    /// Map PID to systemd unit using cgroup path
    fn get_unit_from_pid(pid: u32) -> Option<String> {
        let cgroup_path = format!("/proc/{pid}/cgroup");
        if let Ok(content) = std::fs::read_to_string(cgroup_path) {
            for line in content.lines() {
                // Look for systemd cgroup (typically hierarchy 1 or 0)
                if line.contains("systemd:") || line.contains("name=systemd") {
                    // Extract unit name from path like /system.slice/nginx.service
                    if let Some(path_part) = line.split(':').nth(2) {
                        if let Some(unit_name) = path_part.split('/').next_back() {
                            #[allow(clippy::case_sensitive_file_extension_comparisons)]
                            if unit_name.ends_with(".service")
                                || unit_name.ends_with(".timer")
                                || unit_name.ends_with(".socket")
                            {
                                return Some(unit_name.to_string());
                            }
                        }
                    }
                }
            }
        }
        None
    }
}

impl Default for ProcfsMetricsReader {
    fn default() -> Self {
        Self::new()
    }
}

impl ProcessMetricsReader for ProcfsMetricsReader {
    async fn get_all_process_metrics(&self) -> Result<Vec<ProcessMetrics>> {
        let processes = all_processes().context("Failed to read processes")?;
        let mut metrics = Vec::new();
        let mut reader = self.clone(); // Create mutable clone for CPU calculations

        for process in processes.flatten() {
            if let Ok(metric) = reader.process_to_metrics(&process) {
                metrics.push(metric);
            }
        }

        Ok(metrics)
    }

    async fn get_process_metrics(&self, pid: u32) -> Result<Option<ProcessMetrics>> {
        match Process::new(pid.try_into().unwrap_or(0)) {
            Ok(process) => {
                let mut reader = self.clone();
                match reader.process_to_metrics(&process) {
                    Ok(metrics) => Ok(Some(metrics)),
                    Err(_) => Ok(None),
                }
            }
            Err(_) => Ok(None),
        }
    }

    async fn get_unit_process_metrics(&self, unit_name: &str) -> Result<Vec<ProcessMetrics>> {
        let all_metrics = self.get_all_process_metrics().await?;
        let mut unit_metrics = Vec::new();

        for metric in all_metrics {
            if let Some(process_unit) = Self::get_unit_from_pid(metric.pid) {
                if process_unit == unit_name {
                    unit_metrics.push(metric);
                }
            }
        }

        Ok(unit_metrics)
    }
}

/// Per-unit metrics reader that aggregates process metrics
#[derive(Clone)]
pub struct UnitMetricsCollector {
    process_reader: ProcfsMetricsReader,
}

impl UnitMetricsCollector {
    /// Create a new unit metrics collector
    #[must_use]
    pub fn new() -> Self {
        Self {
            process_reader: ProcfsMetricsReader::new(),
        }
    }

    /// Aggregate process metrics into unit metrics
    #[allow(clippy::cast_possible_wrap)]
    fn aggregate_process_metrics(unit_name: String, processes: Vec<ProcessMetrics>) -> UnitMetrics {
        let mut total_cpu = 0.0;
        let mut total_memory_rss = 0;
        let mut total_memory_vsz = 0;
        let mut total_io_read = 0;
        let mut total_io_write = 0;
        let mut total_net_rx = 0;
        let mut total_net_tx = 0;
        let mut pids = Vec::new();

        for process in processes {
            total_cpu += process.cpu_usage;
            total_memory_rss += process.memory_rss;
            total_memory_vsz += process.memory_vsz;
            total_io_read += process.io_read_bps;
            total_io_write += process.io_write_bps;
            total_net_rx += process.net_rx_bps;
            total_net_tx += process.net_tx_bps;
            pids.push(process.pid);
        }

        UnitMetrics {
            unit_name,
            cpu_usage: total_cpu,
            memory_rss: total_memory_rss,
            memory_vsz: total_memory_vsz,
            io_read_bps: total_io_read,
            io_write_bps: total_io_write,
            net_rx_bps: total_net_rx,
            net_tx_bps: total_net_tx,
            pids,
        }
    }
}

impl Default for UnitMetricsCollector {
    fn default() -> Self {
        Self::new()
    }
}

impl UnitMetricsReader for UnitMetricsCollector {
    async fn get_all_unit_metrics(&self) -> Result<Vec<UnitMetrics>> {
        let all_processes = self.process_reader.get_all_process_metrics().await?;
        let mut units: HashMap<String, Vec<ProcessMetrics>> = HashMap::new();

        // Group processes by unit
        for process in all_processes {
            if let Some(unit_name) = ProcfsMetricsReader::get_unit_from_pid(process.pid) {
                units.entry(unit_name).or_default().push(process);
            }
        }

        // Aggregate metrics for each unit
        let mut unit_metrics = Vec::new();
        for (unit_name, processes) in units {
            let metrics = Self::aggregate_process_metrics(unit_name, processes);
            unit_metrics.push(metrics);
        }

        Ok(unit_metrics)
    }

    async fn get_unit_metrics(&self, unit_name: &str) -> Result<Option<UnitMetrics>> {
        let processes = self
            .process_reader
            .get_unit_process_metrics(unit_name)
            .await?;

        if processes.is_empty() {
            Ok(None)
        } else {
            let metrics = Self::aggregate_process_metrics(unit_name.to_string(), processes);
            Ok(Some(metrics))
        }
    }
}

// Clone implementations for CPU time cache management
impl Clone for ProcfsMetricsReader {
    fn clone(&self) -> Self {
        Self {
            cpu_time_cache: self.cpu_time_cache.clone(),
            io_cache: self.io_cache.clone(),
            net_cache: self.net_cache.clone(),
            cpu_count: self.cpu_count,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_get_all_process_metrics() {
        let reader = ProcfsMetricsReader::new();
        let metrics = reader.get_all_process_metrics().await;

        // Should succeed (even if some processes can't be read)
        assert!(metrics.is_ok());
        let metrics = metrics.unwrap();

        // Should find at least some processes
        assert!(!metrics.is_empty());

        // Verify metrics structure for first process
        if let Some(first) = metrics.first() {
            assert!(first.pid > 0);
            assert!(first.cpu_usage >= 0.0);
            assert!(!first.name.is_empty());
        }
    }

    #[tokio::test]
    async fn test_get_process_metrics() {
        let reader = ProcfsMetricsReader::new();

        // Test with current process PID
        let current_pid = std::process::id();
        let metrics = reader.get_process_metrics(current_pid).await;

        assert!(metrics.is_ok());
        let metrics = metrics.unwrap();
        assert!(metrics.is_some());

        let metrics = metrics.unwrap();
        assert_eq!(metrics.pid, current_pid);
        assert!(metrics.cpu_usage >= 0.0);
        assert!(!metrics.name.is_empty());
    }

    #[tokio::test]
    async fn test_unit_metrics_collector() {
        let collector = UnitMetricsCollector::new();
        let metrics = collector.get_all_unit_metrics().await;

        // Should succeed
        assert!(metrics.is_ok());

        // May or may not find units depending on the system
        let metrics = metrics.unwrap();
        for unit in metrics {
            assert!(!unit.unit_name.is_empty());
            assert!(!unit.pids.is_empty());
            assert!(unit.cpu_usage >= 0.0);
        }
    }
}
