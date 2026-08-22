//! Vitals daemon - System health monitoring
//!
//! A lightweight daemon that monitors system health through journald and systemd,
//! calculating health scores based on errors, warnings, and resource utilization.

// Re-export core types
pub use vitals_core::{
    HealthBreakdown, Issue, IssueError, IssueImpact, IssueTrend, ResourceConsumer,
    ResourceHealthMetrics, ResourceStatus, Severity,
};

pub mod agg;
pub mod config;
/// Data collection and streaming modules for systemd, journald, and metrics
pub mod data;
pub mod health;
pub mod history;
/// Log entry collection and querying for the `/logs` endpoint
pub mod logs;
/// Legacy model module for compatibility
pub mod model;
pub mod notifier;
pub mod probes;
