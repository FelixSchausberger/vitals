//! Mock implementations for testing and development.
//!
//! Provides deterministic mock data for system monitoring components.

use crate::data::traits::{
    JournalEntry, JournalReader, Metrics, MetricsReader, SystemdReader, SystemdUnit,
};

/// Mock journal reader with deterministic test data.
#[derive(Debug, Clone, Default)]
pub struct MockJournal;

/// Mock systemd reader with deterministic test data.
#[derive(Debug, Clone, Default)]
pub struct MockSystemd;

/// Mock metrics reader with deterministic test data.
#[derive(Debug, Clone, Default)]
pub struct MockMetrics;

impl JournalReader for MockJournal {
    async fn read_entries(&self, limit: usize) -> anyhow::Result<Vec<JournalEntry>> {
        self.read_entries_since(None, limit).await
    }

    async fn read_entries_since(
        &self,
        since: Option<time::OffsetDateTime>,
        limit: usize,
    ) -> anyhow::Result<Vec<JournalEntry>> {
        // Use current time for mock entries to ensure they're not filtered out
        let now = time::OffsetDateTime::now_utc();
        let entries = vec![
            JournalEntry {
                timestamp: now - time::Duration::minutes(60),
                priority: 3, // Error
                message: "Critical system error occurred".to_string(),
                unit: Some("nginx.service".to_string()),
                pid: Some(1234),
            },
            JournalEntry {
                timestamp: now - time::Duration::minutes(50),
                priority: 2, // Critical
                message: "Database connection failed".to_string(),
                unit: Some("postgresql.service".to_string()),
                pid: Some(5678),
            },
            JournalEntry {
                timestamp: now - time::Duration::minutes(40),
                priority: 4, // Warning
                message: "High memory usage detected".to_string(),
                unit: Some("memory-monitor.service".to_string()),
                pid: Some(9012),
            },
            JournalEntry {
                timestamp: now - time::Duration::minutes(30),
                priority: 4, // Warning
                message: "Disk space running low".to_string(),
                unit: Some("disk-monitor.service".to_string()),
                pid: Some(3456),
            },
            JournalEntry {
                timestamp: now - time::Duration::minutes(20),
                priority: 6, // Info
                message: "Service started successfully".to_string(),
                unit: Some("app.service".to_string()),
                pid: Some(7890),
            },
            JournalEntry {
                timestamp: now - time::Duration::minutes(10),
                priority: 1, // Emergency
                message: "System panic detected".to_string(),
                unit: Some("kernel".to_string()),
                pid: Some(0),
            },
        ];

        // Filter by time if since parameter is provided
        let filtered_entries: Vec<_> = if let Some(since_time) = since {
            entries
                .into_iter()
                .filter(|entry| entry.timestamp >= since_time)
                .collect()
        } else {
            entries
        };

        Ok(filtered_entries.into_iter().take(limit).collect())
    }

    async fn read_unit_entries(
        &self,
        unit: &str,
        limit: usize,
    ) -> anyhow::Result<Vec<JournalEntry>> {
        let all_entries = self.read_entries(100).await?;
        Ok(all_entries
            .into_iter()
            .filter(|entry| entry.unit.as_ref().is_some_and(|u| u == unit))
            .take(limit)
            .collect())
    }
}

impl SystemdReader for MockSystemd {
    async fn list_units(&self) -> anyhow::Result<Vec<SystemdUnit>> {
        // Mock data provides realistic service scenarios for demonstration
        // These failures are intentional to show how the TUI handles various service states
        Ok(vec![
            SystemdUnit {
                name: "nginx.service".to_string(),
                active_state: "active".to_string(),
                load_state: "loaded".to_string(),
                sub_state: "running".to_string(),
                description: "The nginx HTTP and reverse proxy server".to_string(),
                pids: vec![1234, 1235],
            },
            SystemdUnit {
                name: "postgresql.service".to_string(),
                active_state: "failed".to_string(), // Intentionally failed for demo
                load_state: "loaded".to_string(),
                sub_state: "failed".to_string(),
                description: "PostgreSQL database server".to_string(),
                pids: vec![],
            },
            SystemdUnit {
                name: "ssh.service".to_string(),
                active_state: "active".to_string(),
                load_state: "loaded".to_string(),
                sub_state: "running".to_string(),
                description: "OpenSSH server daemon".to_string(),
                pids: vec![678],
            },
        ])
    }

    async fn get_unit(&self, name: &str) -> anyhow::Result<Option<SystemdUnit>> {
        let units = self.list_units().await?;
        Ok(units.into_iter().find(|unit| unit.name == name))
    }
}

impl MetricsReader for MockMetrics {
    async fn get_metrics(&self) -> anyhow::Result<Metrics> {
        Ok(Metrics {
            cpu_usage: 75.5,
            memory_usage: 82.3,
            disk_usage: 45.7,
            load_average: 1.25,
        })
    }
}
