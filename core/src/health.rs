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
    /// Current health score (0-100). Identical to `overall_score`.
    ///
    /// Historical note: this field was originally intended for EWMA smoothing,
    /// but temporal weighting is now handled at the scorer algorithm level
    /// (frecency decay with configurable half-life). No additional smoothing
    /// is applied here.
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
    /// CPU penalty gauge in 0..=`r_max`, drives `cpu_status`
    pub cpu_penalty: f64,
    /// CPU share of the weighted resource burden (`alpha_cpu` × `cpu_penalty`)
    pub cpu_burden: f64,
    /// CPU usage health status
    pub cpu_status: ResourceStatus,
    /// Current memory usage percentage
    pub memory_usage: f64,
    /// Memory penalty gauge in 0..=`r_max`, drives `memory_status`
    pub memory_penalty: f64,
    /// Memory share of the weighted resource burden
    pub memory_burden: f64,
    /// Memory usage health status
    pub memory_status: ResourceStatus,
    /// Current disk usage percentage
    pub disk_usage: f64,
    /// Disk penalty gauge in 0..=`r_max`, drives `disk_status`
    pub disk_penalty: f64,
    /// Disk share of the weighted resource burden
    pub disk_burden: f64,
    /// Disk usage health status
    pub disk_status: ResourceStatus,
    /// Current load average
    pub load_average: f64,
    /// Load penalty gauge in 0..=`r_max`, drives `load_status`
    pub load_penalty: f64,
    /// Load share of the weighted resource burden
    pub load_burden: f64,
    /// Load average health status
    pub load_status: ResourceStatus,
    /// Total resource impact on health score (negative value)
    pub resource_impact: f64,
    /// Number of resource hogs (high-utilization units)
    pub resource_hog_count: usize,
    /// Top resource-consuming units
    pub top_resource_consumers: Vec<ResourceConsumer>,
    /// True during the baseline learning period (first N samples)
    #[serde(default)]
    pub baseline_learning_mode: bool,
    /// Number of samples collected in the resource baseline
    #[serde(default)]
    pub baseline_samples: u64,
    /// Samples needed before baseline is considered reliable
    #[serde(default)]
    pub min_samples_for_baseline: u64,
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
    /// Associated systemd unit, if applicable
    #[serde(default)]
    pub unit: Option<String>,
    /// Actionable hints for resolving the issue
    #[serde(default)]
    pub hints: Vec<String>,
}
