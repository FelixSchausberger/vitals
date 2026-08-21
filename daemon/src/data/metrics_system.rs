//! System-wide metrics collection using procfs.
//!
//! Replaces the former sysinfo-based reader: CPU, memory, load average, and
//! (real) disk usage are all derived from `/proc`, keeping the daemon on a
//! single system-metrics dependency for its Linux-only target.

use std::collections::HashSet;

use anyhow::Result;
use procfs::{Current, CurrentSI, KernelStats, LoadAverage, Meminfo};
use tokio::sync::Mutex;

use crate::data::traits::{Metrics, MetricsReader};

/// Filesystems counted toward disk usage.
const PHYSICAL_FS_TYPES: [&str; 7] = ["ext2", "ext3", "ext4", "xfs", "btrfs", "zfs", "f2fs"];

/// Real system metrics reader based on procfs.
pub struct ProcfsSystemMetricsReader {
    /// Serializes the two-sample CPU measurement.
    cpu_lock: Mutex<()>,
}

impl ProcfsSystemMetricsReader {
    /// Create a new reader.
    #[must_use]
    pub fn new() -> Self {
        Self {
            cpu_lock: Mutex::new(()),
        }
    }

    /// Two-sample CPU utilization: total busy jiffies delta over the sample
    /// window, matching the behaviour of the previous sysinfo reader.
    async fn cpu_usage(&self) -> Result<f64> {
        let _guard = self.cpu_lock.lock().await;

        let first = total_busy_jiffies()?;
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        let second = total_busy_jiffies()?;

        let delta = second.saturating_sub(first);
        if delta == 0 {
            return Ok(0.0);
        }

        // Jiffies accumulate across all cores; normalize to a 0-100 scale.
        let cores = num_cpus::get();
        #[allow(clippy::cast_precision_loss)]
        let window_jiffies = (ticks_per_second() as f64) * 0.1 * cores as f64;
        #[allow(clippy::cast_precision_loss)]
        let usage = (delta as f64 / window_jiffies) * 100.0;
        Ok(usage.clamp(0.0, 100.0))
    }
}

impl Default for ProcfsSystemMetricsReader {
    fn default() -> Self {
        Self::new()
    }
}

/// Sum of non-idle CPU jiffies across all cores.
fn total_busy_jiffies() -> Result<u64> {
    let stats = KernelStats::current()?;
    let t = &stats.total;
    // Idle time (idle + iowait) is excluded from "busy".
    let idle = t.idle + t.iowait.unwrap_or(0);
    let total: u64 = t.user
        + t.nice
        + t.system
        + t.idle
        + t.iowait.unwrap_or(0)
        + t.irq.unwrap_or(0)
        + t.softirq.unwrap_or(0)
        + t.steal.unwrap_or(0);
    Ok(total.saturating_sub(idle))
}

/// Memory utilization percentage from `/proc/meminfo`.
fn memory_usage() -> Result<f64> {
    let info = Meminfo::current()?;
    let total = info.mem_total;
    let available = info.mem_available.unwrap_or(info.mem_free);
    if total == 0 {
        return Ok(0.0);
    }
    #[allow(clippy::cast_precision_loss)]
    let pct = ((total - available) as f64 / total as f64) * 100.0;
    Ok(pct.clamp(0.0, 100.0))
}

/// Aggregate disk usage over device-backed filesystems from `/proc/mounts`.
///
/// Bind mounts and duplicate devices are de-duplicated by device name so the
/// same filesystem is not counted twice.
#[must_use]
pub fn disk_usage() -> f64 {
    let Ok(mounts) = std::fs::read_to_string("/proc/mounts") else {
        return 0.0;
    };

    let mut seen_devices: HashSet<&str> = HashSet::new();
    let mut total_bytes: u128 = 0;
    let mut used_bytes: u128 = 0;

    for line in mounts.lines() {
        let mut fields = line.split_whitespace();
        let (Some(device), Some(mount_point), Some(fs_type)) =
            (fields.next(), fields.next(), fields.next())
        else {
            continue;
        };
        if !PHYSICAL_FS_TYPES.contains(&fs_type) || !seen_devices.insert(device) {
            continue;
        }
        if let Some((total, used)) = statvfs_usage(mount_point) {
            total_bytes += u128::from(total);
            used_bytes += u128::from(used);
        }
    }

    if total_bytes == 0 {
        return 0.0;
    }
    #[allow(clippy::cast_precision_loss)]
    let pct = (used_bytes as f64 / total_bytes as f64) * 100.0;
    pct.clamp(0.0, 100.0)
}

/// `(total_bytes, used_bytes)` for a mount point via statvfs.
fn statvfs_usage(mount_point: &str) -> Option<(u64, u64)> {
    let stat = nix::sys::statvfs::statvfs(mount_point).ok()?;
    let block_size = stat.fragment_size();
    let total = stat.blocks().saturating_mul(block_size);
    let free = stat.blocks_free().saturating_mul(block_size);
    Some((total, total.saturating_sub(free)))
}

impl MetricsReader for ProcfsSystemMetricsReader {
    async fn get_metrics(&self) -> Result<Metrics> {
        let cpu_usage = self.cpu_usage().await?;
        let load_average = f64::from(LoadAverage::current()?.one);

        Ok(Metrics {
            cpu_usage,
            memory_usage: memory_usage().unwrap_or(0.0),
            disk_usage: disk_usage(),
            load_average,
        })
    }
}

fn ticks_per_second() -> u64 {
    procfs::ticks_per_second()
}

/// Seconds since boot, from `/proc/uptime`.
///
/// # Errors
///
/// Returns an error if `/proc/uptime` cannot be read.
pub fn uptime_secs() -> Result<u64> {
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        clippy::cast_precision_loss
    )]
    let secs = procfs::Uptime::current()?.uptime as u64;
    Ok(secs)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_get_metrics() {
        let reader = ProcfsSystemMetricsReader::new();
        let metrics = reader.get_metrics().await.expect("Failed to get metrics");

        assert!((0.0..=100.0).contains(&metrics.cpu_usage));
        assert!((0.0..=100.0).contains(&metrics.memory_usage));
        assert!((0.0..=100.0).contains(&metrics.disk_usage));
        assert!(metrics.load_average >= 0.0);
    }

    #[tokio::test]
    async fn test_reuse_across_calls() {
        let reader = ProcfsSystemMetricsReader::new();

        for _ in 0..3 {
            let metrics = reader.get_metrics().await.expect("Failed to get metrics");
            assert!((0.0..=100.0).contains(&metrics.cpu_usage));
            assert!((0.0..=100.0).contains(&metrics.memory_usage));
        }
    }

    #[test]
    fn test_disk_usage_is_real_on_physical_hosts() {
        let usage = disk_usage();
        // On a normal Linux host with at least one physical filesystem this
        // must be strictly positive; the old sysinfo reader reported 0.0 here.
        assert!(usage > 0.0, "disk usage should be measured, got {usage}");
        assert!(usage <= 100.0);
    }

    #[test]
    fn test_busy_jiffies_positive() {
        let busy = total_busy_jiffies().expect("kernel stats");
        assert!(busy > 0);
    }
}
