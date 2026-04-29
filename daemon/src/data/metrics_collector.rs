//! Metrics collection and storage system.
//!
//! This module provides a unified system for collecting, storing, and querying
//! time series metrics data with configurable sampling intervals and storage options.

#![allow(dead_code)]

use std::{collections::HashMap, sync::Arc};

use anyhow::Result;
use time::OffsetDateTime;
use tokio::{
    sync::RwLock,
    time::{interval, Duration},
};

use crate::data::traits::{
    MetricsConfig, MetricsDataPoint, MetricsRingBuffer, ProcessMetrics, ProcessMetricsReader,
    UnitMetrics, UnitMetricsReader,
};

/// Comprehensive metrics collector that manages all metrics types
pub struct MetricsCollector {
    /// Configuration for metrics collection
    config: MetricsConfig,
    /// Process metrics reader
    process_reader: crate::data::metrics_procfs::ProcfsMetricsReader,
    /// Unit metrics reader
    unit_reader: crate::data::metrics_procfs::UnitMetricsCollector,
    /// Storage for process metrics (PID -> ring buffer)
    process_metrics: Arc<RwLock<HashMap<u32, MetricsRingBuffer<ProcessMetrics>>>>,
    /// Storage for unit metrics (unit name -> ring buffer)
    unit_metrics: Arc<RwLock<HashMap<String, MetricsRingBuffer<UnitMetrics>>>>,
    /// Whether the collector is currently running
    is_running: Arc<RwLock<bool>>,
}

impl MetricsCollector {
    /// Create a new metrics collector
    #[must_use]
    pub fn new(config: MetricsConfig) -> Self {
        Self {
            config,
            process_reader: crate::data::metrics_procfs::ProcfsMetricsReader::new(),
            unit_reader: crate::data::metrics_procfs::UnitMetricsCollector::new(),
            process_metrics: Arc::new(RwLock::new(HashMap::new())),
            unit_metrics: Arc::new(RwLock::new(HashMap::new())),
            is_running: Arc::new(RwLock::new(false)),
        }
    }

    /// Start the metrics collection loop
    ///
    /// # Errors
    /// Returns an error if the metrics collection fails to start or encounters an error during operation
    pub async fn start(&self) -> Result<()> {
        let mut is_running = self.is_running.write().await;
        if *is_running {
            return Ok(()); // Already running
        }
        *is_running = true;
        drop(is_running);

        let process_reader = self.process_reader.clone();
        let unit_reader = self.unit_reader.clone();
        let process_metrics = Arc::clone(&self.process_metrics);
        let unit_metrics = Arc::clone(&self.unit_metrics);
        let is_running_flag = Arc::clone(&self.is_running);
        let interval_duration = Duration::from_secs(self.config.sample_interval_secs);
        let buffer_size = self.config.memory_buffer_size;

        tokio::spawn(async move {
            let mut ticker = interval(interval_duration);

            loop {
                // Check if we should continue running
                {
                    let running = is_running_flag.read().await;
                    if !*running {
                        break;
                    }
                }

                ticker.tick().await;

                let timestamp = OffsetDateTime::now_utc();

                // Collect process metrics
                if let Ok(processes) = process_reader.get_all_process_metrics().await {
                    let mut process_storage = process_metrics.write().await;

                    for process in &processes {
                        let data_point = MetricsDataPoint {
                            timestamp,
                            data: process.clone(),
                        };

                        process_storage
                            .entry(process.pid)
                            .or_insert_with(|| MetricsRingBuffer::new(buffer_size))
                            .push(data_point);
                    }

                    // Clean up old PIDs that no longer exist
                    let current_pids: std::collections::HashSet<u32> =
                        processes.iter().map(|p| p.pid).collect();
                    process_storage.retain(|&pid, _| current_pids.contains(&pid));
                }

                // Collect unit metrics
                if let Ok(units) = unit_reader.get_all_unit_metrics().await {
                    let mut unit_storage = unit_metrics.write().await;

                    for unit in &units {
                        let data_point = MetricsDataPoint {
                            timestamp,
                            data: unit.clone(),
                        };

                        unit_storage
                            .entry(unit.unit_name.clone())
                            .or_insert_with(|| MetricsRingBuffer::new(buffer_size))
                            .push(data_point);
                    }

                    // Clean up units with no active processes
                    let current_units: std::collections::HashSet<String> =
                        units.iter().map(|u| u.unit_name.clone()).collect();
                    unit_storage.retain(|unit_name, _| current_units.contains(unit_name));
                }
            }
        });

        Ok(())
    }

    /// Stop the metrics collection loop
    pub async fn stop(&self) {
        let mut is_running = self.is_running.write().await;
        *is_running = false;
    }

    /// Get recent process metrics for a specific PID
    pub async fn get_process_metrics_recent(
        &self,
        pid: u32,
        count: usize,
    ) -> Option<Vec<MetricsDataPoint<ProcessMetrics>>> {
        let storage = self.process_metrics.read().await;
        storage
            .get(&pid)
            .map(|buffer| buffer.get_recent(count).into_iter().cloned().collect())
    }

    /// Get process metrics within a time range
    pub async fn get_process_metrics_range(
        &self,
        pid: u32,
        start: OffsetDateTime,
        end: OffsetDateTime,
    ) -> Option<Vec<MetricsDataPoint<ProcessMetrics>>> {
        let storage = self.process_metrics.read().await;
        storage
            .get(&pid)
            .map(|buffer| buffer.get_range(start, end).into_iter().cloned().collect())
    }

    /// Get recent unit metrics for a specific unit
    pub async fn get_unit_metrics_recent(
        &self,
        unit_name: &str,
        count: usize,
    ) -> Option<Vec<MetricsDataPoint<UnitMetrics>>> {
        let storage = self.unit_metrics.read().await;
        storage
            .get(unit_name)
            .map(|buffer| buffer.get_recent(count).into_iter().cloned().collect())
    }

    /// Get unit metrics within a time range
    pub async fn get_unit_metrics_range(
        &self,
        unit_name: &str,
        start: OffsetDateTime,
        end: OffsetDateTime,
    ) -> Option<Vec<MetricsDataPoint<UnitMetrics>>> {
        let storage = self.unit_metrics.read().await;
        storage
            .get(unit_name)
            .map(|buffer| buffer.get_range(start, end).into_iter().cloned().collect())
    }

    /// Get all available process PIDs
    pub async fn get_available_process_pids(&self) -> Vec<u32> {
        let storage = self.process_metrics.read().await;
        storage.keys().copied().collect()
    }

    /// Get all available unit names
    pub async fn get_available_unit_names(&self) -> Vec<String> {
        let storage = self.unit_metrics.read().await;
        storage.keys().cloned().collect()
    }

    /// Check if the collector is running
    pub async fn is_running(&self) -> bool {
        *self.is_running.read().await
    }

    /// Get the current configuration
    #[must_use]
    pub const fn get_config(&self) -> &MetricsConfig {
        &self.config
    }
}
