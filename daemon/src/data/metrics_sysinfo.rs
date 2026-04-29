//! Real system metrics integration using sysinfo.
//!
//! The `System` instance is held across calls and refreshed incrementally so
//! that subsequent `get_metrics()` invocations avoid the cost of `System::new_all()`.

use anyhow::Result;
use sysinfo::System;
use time::OffsetDateTime;
use tokio::sync::Mutex;

use crate::data::traits::{Metrics, MetricsReader};

/// Real metrics reader implementation using sysinfo.
///
/// The underlying `System` handle is kept alive between calls and refreshed
/// incrementally, which avoids re-allocating the full sysinfo data structure
/// on every polling tick.
pub struct SysinfoMetricsReader {
    system: Mutex<System>,
}

impl SysinfoMetricsReader {
    /// Create a new metrics reader, performing an initial full refresh.
    #[must_use]
    pub fn new() -> Self {
        let mut system = System::new_all();
        system.refresh_all();
        Self {
            system: Mutex::new(system),
        }
    }
}

impl MetricsReader for SysinfoMetricsReader {
    async fn get_metrics(&self) -> Result<Metrics> {
        let mut system = self.system.lock().await;

        // Two-sample CPU measurement: refresh, wait 100 ms, refresh again.
        // Using async sleep so we don't block the tokio thread.
        system.refresh_cpu_all();
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        system.refresh_cpu_all();
        system.refresh_memory();

        let cpu_usage = f64::from(system.global_cpu_usage());

        let memory_usage = {
            let total = system.total_memory();
            let used = system.used_memory();
            if total == 0 {
                0.0
            } else {
                #[allow(clippy::cast_precision_loss)]
                let pct = (used as f64 / total as f64) * 100.0;
                pct
            }
        };

        // Load average from /proc/loadavg (Unix only); fall back to CPU-based estimate.
        let load_average = {
            #[cfg(unix)]
            {
                std::fs::read_to_string("/proc/loadavg")
                    .ok()
                    .and_then(|s| s.split_whitespace().next()?.parse::<f64>().ok())
                    .unwrap_or_else(|| {
                        #[allow(clippy::cast_precision_loss)]
                        let cpu_count = system.cpus().len() as f64;
                        (f64::from(system.global_cpu_usage()) / 100.0) * cpu_count
                    })
            }
            #[cfg(not(unix))]
            {
                #[allow(clippy::cast_precision_loss)]
                let cpu_count = system.cpus().len() as f64;
                (f64::from(system.global_cpu_usage()) / 100.0) * cpu_count
            }
        };

        // Disk usage: placeholder — sysinfo disk API changed significantly in 0.32
        let disk_usage = 0.0_f64;

        Ok(Metrics {
            cpu_usage,
            memory_usage,
            disk_usage,
            load_average,
        })
    }

    async fn get_metrics_range(
        &self,
        start: OffsetDateTime,
        end: OffsetDateTime,
    ) -> Result<Vec<(OffsetDateTime, Metrics)>> {
        let _ = end - start;
        let current_time = OffsetDateTime::now_utc();
        if current_time >= start && current_time <= end {
            let metrics = self.get_metrics().await?;
            Ok(vec![(current_time, metrics)])
        } else {
            Ok(Vec::new())
        }
    }
}

impl Default for SysinfoMetricsReader {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_get_metrics() {
        let reader = SysinfoMetricsReader::new();
        let metrics = reader.get_metrics().await.expect("Failed to get metrics");

        assert!((0.0..=100.0).contains(&metrics.cpu_usage));
        assert!((0.0..=100.0).contains(&metrics.memory_usage));
        assert!((0.0..=100.0).contains(&metrics.disk_usage));
        assert!(metrics.load_average >= 0.0);
    }

    #[tokio::test]
    async fn test_reuse_across_calls() {
        let reader = SysinfoMetricsReader::new();

        // Multiple calls should all succeed and not rebuild System::new_all()
        for _ in 0..3 {
            let metrics = reader.get_metrics().await.expect("Failed to get metrics");
            assert!((0.0..=100.0).contains(&metrics.cpu_usage));
            assert!((0.0..=100.0).contains(&metrics.memory_usage));
        }
    }

    #[tokio::test]
    async fn test_get_metrics_range() {
        let reader = SysinfoMetricsReader::new();
        let start = OffsetDateTime::now_utc() - time::Duration::minutes(1);
        let end = OffsetDateTime::now_utc() + time::Duration::minutes(1);

        let range = reader
            .get_metrics_range(start, end)
            .await
            .expect("Failed to get metrics range");

        assert_eq!(range.len(), 1);
        let (ts, metrics) = &range[0];
        assert!(*ts >= start && *ts <= end);
        assert!((0.0..=100.0).contains(&metrics.cpu_usage));
    }
}
