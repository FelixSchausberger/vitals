//! Data traits for system monitoring components.
//!
//! This module defines the core traits and data structures for reading
//! system information from various sources like systemd and journald.

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

/// A journal entry from systemd's journal.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct JournalEntry {
    /// ISO 8601 timestamp when the entry was created
    #[serde(with = "time::serde::rfc3339")]
    pub timestamp: OffsetDateTime,
    /// Syslog priority level (0-7, lower is more critical)
    pub priority: u8,
    /// The log message content
    pub message: String,
    /// Associated systemd unit name if available
    pub unit: Option<String>,
    /// Process ID that generated the log entry
    pub pid: Option<u32>,
}

/// A systemd unit and its current state.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SystemdUnit {
    /// Unit name (e.g., "nginx.service")
    pub name: String,
    /// Current active state ("active", "inactive", "failed", etc.)
    pub active_state: String,
    /// Current load state ("loaded", "not-found", etc.)
    pub load_state: String,
    /// Current sub-state ("running", "dead", "failed", etc.)
    pub sub_state: String,
    /// Human-readable description of the unit
    pub description: String,
    /// Currently running process IDs for this unit
    pub pids: Vec<u32>,
}

/// System performance metrics.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Metrics {
    /// CPU usage percentage (0.0-100.0)
    pub cpu_usage: f64,
    /// Memory usage percentage (0.0-100.0)
    pub memory_usage: f64,
    /// Disk usage percentage (0.0-100.0)
    pub disk_usage: f64,
    /// System load average (1-minute)
    pub load_average: f64,
}

/// Per-process metrics for detailed monitoring.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ProcessMetrics {
    /// Process ID
    pub pid: u32,
    /// CPU usage percentage (0.0-100.0)
    pub cpu_usage: f64,
    /// Memory usage in bytes (RSS)
    pub memory_rss: u64,
    /// Memory usage in bytes (Virtual)
    pub memory_vsz: u64,
    /// IO read bytes per second
    pub io_read_bps: u64,
    /// IO write bytes per second
    pub io_write_bps: u64,
    /// Network receive bytes per second
    pub net_rx_bps: u64,
    /// Network transmit bytes per second
    pub net_tx_bps: u64,
    /// Process name
    pub name: String,
    /// Command line
    pub cmdline: String,
}

/// Per-unit aggregated metrics from all processes in the unit.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct UnitMetrics {
    /// Unit name (e.g., "nginx.service")
    pub unit_name: String,
    /// Combined CPU usage percentage from all processes
    pub cpu_usage: f64,
    /// Combined memory usage in bytes (RSS)
    pub memory_rss: u64,
    /// Combined memory usage in bytes (Virtual)
    pub memory_vsz: u64,
    /// Combined IO read bytes per second
    pub io_read_bps: u64,
    /// Combined IO write bytes per second
    pub io_write_bps: u64,
    /// Combined network receive bytes per second
    pub net_rx_bps: u64,
    /// Combined network transmit bytes per second
    pub net_tx_bps: u64,
    /// List of PIDs belonging to this unit
    pub pids: Vec<u32>,
}

/// Async trait for reading journal entries.
#[allow(async_fn_in_trait)]
pub trait JournalReader {
    /// Read journal entries with optional limit.
    ///
    /// Returns the most recent entries up to the specified limit.
    async fn read_entries(&self, limit: usize) -> anyhow::Result<Vec<JournalEntry>>;

    /// Read journal entries with time filtering.
    ///
    /// Returns entries since the specified time, up to the limit.
    async fn read_entries_since(
        &self,
        since: Option<OffsetDateTime>,
        limit: usize,
    ) -> anyhow::Result<Vec<JournalEntry>>;

    /// Read journal entries for a specific unit.
    async fn read_unit_entries(
        &self,
        unit: &str,
        limit: usize,
    ) -> anyhow::Result<Vec<JournalEntry>>;
}

/// Async trait for reading systemd unit information.
#[allow(async_fn_in_trait)]
pub trait SystemdReader {
    /// List all systemd units with their current states.
    async fn list_units(&self) -> anyhow::Result<Vec<SystemdUnit>>;

    /// Get information about a specific unit.
    async fn get_unit(&self, name: &str) -> anyhow::Result<Option<SystemdUnit>>;
}

/// Async trait for reading system metrics.
#[allow(async_fn_in_trait)]
pub trait MetricsReader {
    /// Get current system performance metrics.
    async fn get_metrics(&self) -> anyhow::Result<Metrics>;

    /// Get metrics for a specific time range (future enhancement).
    async fn get_metrics_range(
        &self,
        _start: OffsetDateTime,
        _end: OffsetDateTime,
    ) -> anyhow::Result<Vec<(OffsetDateTime, Metrics)>> {
        // Default implementation returns current metrics
        let metrics = self.get_metrics().await?;
        Ok(vec![(OffsetDateTime::now_utc(), metrics)])
    }
}

/// Async trait for reading per-process metrics.
#[allow(async_fn_in_trait)]
pub trait ProcessMetricsReader {
    /// Get current metrics for all processes.
    async fn get_all_process_metrics(&self) -> anyhow::Result<Vec<ProcessMetrics>>;

    /// Get metrics for a specific process by PID.
    async fn get_process_metrics(&self, pid: u32) -> anyhow::Result<Option<ProcessMetrics>>;

    /// Get metrics for processes belonging to a specific unit.
    async fn get_unit_process_metrics(
        &self,
        unit_name: &str,
    ) -> anyhow::Result<Vec<ProcessMetrics>>;
}

/// Async trait for reading per-unit aggregated metrics.
#[allow(async_fn_in_trait)]
pub trait UnitMetricsReader {
    /// Get current metrics for all units with running processes.
    async fn get_all_unit_metrics(&self) -> anyhow::Result<Vec<UnitMetrics>>;

    /// Get metrics for a specific unit.
    async fn get_unit_metrics(&self, unit_name: &str) -> anyhow::Result<Option<UnitMetrics>>;
}
