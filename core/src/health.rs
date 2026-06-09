//! Health scoring data structures for vitals.
//!
//! Defines health-related data structures for API responses and serialization.

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

use crate::issue::Severity;

/// Health score breakdown by category
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthBreakdown {
    /// Overall health score (0-100)
    pub overall_score: f64,
    /// EWMA smoothed score (0-100)
    pub smoothed_score: f64,
    /// Number of error-level issues
    pub error_count: usize,
    /// Number of warning-level issues
    pub warning_count: usize,
    /// Number of info-level issues
    pub info_count: usize,
    /// Total number of issues
    pub total_issues: usize,
    /// Timestamp of this health calculation
    pub timestamp: OffsetDateTime,
    /// Individual issue contributions to score
    pub issue_impacts: Vec<IssueImpact>,
    /// Resource utilization metrics
    pub resource_metrics: Option<ResourceHealthMetrics>,
}

/// Resource health metrics and their impact on system health
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourceHealthMetrics {
    /// Current CPU usage percentage
    pub cpu_usage: f64,
    /// CPU contribution to resource burden (positive value)
    pub cpu_penalty: f64,
    /// CPU usage health status
    pub cpu_status: ResourceStatus,
    /// Current memory usage percentage
    pub memory_usage: f64,
    /// Memory contribution to resource burden (positive value)
    pub memory_penalty: f64,
    /// Memory usage health status
    pub memory_status: ResourceStatus,
    /// Current disk usage percentage
    pub disk_usage: f64,
    /// Disk contribution to resource burden (positive value)
    pub disk_penalty: f64,
    /// Disk usage health status
    pub disk_status: ResourceStatus,
    /// Current load average
    pub load_average: f64,
    /// Load contribution to resource burden (positive value)
    pub load_penalty: f64,
    /// Load average health status
    pub load_status: ResourceStatus,
    /// Total resource impact on health score (negative value)
    pub resource_impact: f64,
    /// Number of resource hogs (high-utilization units)
    pub resource_hog_count: usize,
    /// Top resource-consuming units
    pub top_resource_consumers: Vec<ResourceConsumer>,
}

/// Resource health status levels
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum ResourceStatus {
    /// Resource usage is healthy
    Healthy,
    /// Resource usage is elevated but acceptable
    Warning,
    /// Resource usage is critically high
    Critical,
}

impl std::fmt::Display for ResourceStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Healthy => write!(f, "Healthy"),
            Self::Warning => write!(f, "Warning"),
            Self::Critical => write!(f, "Critical"),
        }
    }
}

/// Information about a high-resource-consuming unit
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourceConsumer {
    /// Unit name
    pub unit_name: String,
    /// CPU usage percentage
    pub cpu_usage: f64,
    /// Memory usage in MB
    pub memory_mb: u64,
    /// Resource impact score
    pub impact_score: f64,
}

/// Impact of a single issue on health score
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IssueImpact {
    /// Issue ID
    pub id: String,
    /// Issue title
    pub title: String,
    /// Issue severity
    pub severity: Severity,
    /// Issue count/frequency
    pub count: usize,
    /// Score impact (negative value)
    pub impact: f64,
}
