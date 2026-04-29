//! Vitals Core - Shared models and serialization
//!
//! This crate provides shared data structures for the vitals system monitoring suite,
//! including models for issues, health metrics, and API responses.

pub mod api;
pub mod health;
pub mod issue;

pub use api::{HealthResponse, LogEntry, LogsResponse, MetricsSummary};
pub use health::{
    HealthBreakdown, IssueImpact, ResourceConsumer, ResourceHealthMetrics, ResourceStatus,
};
pub use issue::{Issue, IssueError, IssueTrend, Severity};
