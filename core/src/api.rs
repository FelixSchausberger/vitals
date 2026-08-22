//! API models for vitals daemon HTTP endpoints.
//!
//! Defines request and response structures for the daemon's JSON API.

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

use crate::{health::HealthBreakdown, issue::Severity};

/// Response from /health endpoint
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthResponse {
    /// Health status (excellent, good, fair, poor, critical)
    pub status: String,
    /// Overall health score (0-100)
    pub score: f64,
    /// Raw (unsmoothed) health score
    pub raw_score: f64,
    /// Heartbeat color (green, yellow, red)
    pub heartbeat: String,
    /// Unix timestamp of calculation
    pub timestamp: i64,
    /// Issue breakdown
    pub breakdown: IssueBreakdown,
    /// List of issues with impact details
    pub issues: Vec<IssueImpact>,
    /// Resource metrics if available
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resources: Option<ResourceMetrics>,
}

/// Issue breakdown counts
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IssueBreakdown {
    /// Number of error-level issues
    pub errors: usize,
    /// Number of warning-level issues
    pub warnings: usize,
    /// Number of info-level issues
    pub info: usize,
    /// Total number of issues
    pub total: usize,
}

/// Issue impact information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IssueImpact {
    /// Issue ID
    pub id: String,
    /// Issue title
    pub title: String,
    /// Issue severity
    pub severity: Severity,
    /// Occurrence count
    pub count: usize,
    /// Score impact (negative)
    pub impact: f64,
}

/// Resource metrics for /health endpoint
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourceMetrics {
    /// CPU usage percentage
    pub cpu_usage: f64,
    /// CPU status (Healthy, Warning, Critical)
    pub cpu_status: String,
    /// Memory usage percentage
    pub memory_usage: f64,
    /// Memory status
    pub memory_status: String,
    /// Disk usage percentage
    pub disk_usage: f64,
    /// Disk status
    pub disk_status: String,
    /// Load average
    pub load_average: f64,
    /// Load status
    pub load_status: String,
    /// Resource impact on score
    pub resource_impact: f64,
    /// Number of resource hogs
    pub resource_hog_count: usize,
    /// Top resource consumers
    pub top_consumers: Vec<TopConsumer>,
}

/// Top resource consumer information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TopConsumer {
    /// Unit name
    pub unit: String,
    /// CPU usage percentage
    pub cpu_usage: f64,
    /// Memory usage in MB
    pub memory_mb: u64,
    /// Impact score
    pub impact_score: f64,
}

/// Severity filter values accepted as a `/logs` query parameter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SeverityFilter {
    /// Priority 0-3 entries
    Error,
    /// Priority 4 entries
    Warning,
    /// Priority 5-7 entries
    Info,
}

impl From<SeverityFilter> for Severity {
    fn from(filter: SeverityFilter) -> Self {
        match filter {
            SeverityFilter::Error => Self::Error,
            SeverityFilter::Warning => Self::Warning,
            SeverityFilter::Info => Self::Info,
        }
    }
}

/// Query parameters for the `/logs` endpoint.
///
/// All fields are optional; omitted fields do not constrain the result.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LogsQuery {
    /// Only return entries with this severity
    #[serde(default)]
    pub severity: Option<SeverityFilter>,
    /// Only return entries from this systemd unit (exact match)
    #[serde(default)]
    pub unit: Option<String>,
    /// Only return entries at or after this time (RFC 3339)
    #[serde(default, with = "time::serde::rfc3339::option")]
    pub since: Option<OffsetDateTime>,
    /// Only return entries at or before this time (RFC 3339)
    #[serde(default, with = "time::serde::rfc3339::option")]
    pub until: Option<OffsetDateTime>,
    /// Maximum number of entries to return (after `offset`)
    #[serde(default)]
    pub limit: Option<usize>,
    /// Number of matching entries to skip before returning (for pagination)
    #[serde(default)]
    pub offset: usize,
}

/// Response from /logs endpoint
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogsResponse {
    /// Total number of log entries matching criteria
    pub total: usize,
    /// Log entries
    pub entries: Vec<LogEntry>,
    /// Time window for these logs
    pub time_window: TimeWindow,
}

/// Time window specification
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TimeWindow {
    /// Start of time window (ISO 8601)
    #[serde(with = "time::serde::rfc3339")]
    pub start: OffsetDateTime,
    /// End of time window (ISO 8601)
    #[serde(with = "time::serde::rfc3339")]
    pub end: OffsetDateTime,
    /// Human-readable description (e.g., "last 5 minutes", "since boot")
    pub description: String,
}

/// Log entry from journal
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogEntry {
    /// Timestamp
    #[serde(with = "time::serde::rfc3339")]
    pub timestamp: OffsetDateTime,
    /// Log message
    pub message: String,
    /// Severity level
    pub severity: Severity,
    /// Systemd unit (if applicable)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unit: Option<String>,
    /// Process ID
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pid: Option<u32>,
    /// Occurrence count (for aggregated entries)
    #[serde(default = "default_count")]
    pub count: usize,
    /// First seen (for aggregated entries)
    #[serde(
        with = "time::serde::rfc3339::option",
        skip_serializing_if = "Option::is_none"
    )]
    pub first_seen: Option<OffsetDateTime>,
    /// Last seen (for aggregated entries)
    #[serde(
        with = "time::serde::rfc3339::option",
        skip_serializing_if = "Option::is_none"
    )]
    pub last_seen: Option<OffsetDateTime>,
}

fn default_count() -> usize {
    1
}

/// Metrics summary for TUI display
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetricsSummary {
    /// CPU usage percentage
    pub cpu_percent: f64,
    /// Memory usage percentage
    pub memory_percent: f64,
    /// Disk usage percentage
    pub disk_percent: f64,
    /// Load average (1 minute)
    pub load_average: f64,
}

impl From<&HealthBreakdown> for HealthResponse {
    fn from(breakdown: &HealthBreakdown) -> Self {
        let status = score_to_status(breakdown.smoothed_score);
        let heartbeat = score_to_heartbeat(breakdown.smoothed_score);

        let breakdown_struct = IssueBreakdown {
            errors: breakdown.error_count,
            warnings: breakdown.warning_count,
            info: breakdown.info_count,
            total: breakdown.total_issues,
        };

        let issues = breakdown
            .issue_impacts
            .iter()
            .map(|impact| IssueImpact {
                id: impact.id.clone(),
                title: impact.title.clone(),
                severity: impact.severity,
                count: impact.count,
                impact: impact.impact,
            })
            .collect();

        let resources = breakdown
            .resource_metrics
            .as_ref()
            .map(|rm| ResourceMetrics {
                cpu_usage: rm.cpu_usage,
                cpu_status: rm.cpu_status.to_string(),
                memory_usage: rm.memory_usage,
                memory_status: rm.memory_status.to_string(),
                disk_usage: rm.disk_usage,
                disk_status: rm.disk_status.to_string(),
                load_average: rm.load_average,
                load_status: rm.load_status.to_string(),
                resource_impact: rm.resource_impact,
                resource_hog_count: rm.resource_hog_count,
                top_consumers: rm
                    .top_resource_consumers
                    .iter()
                    .map(|c| TopConsumer {
                        unit: c.unit_name.clone(),
                        cpu_usage: c.cpu_usage,
                        memory_mb: c.memory_mb,
                        impact_score: c.impact_score,
                    })
                    .collect(),
            });

        Self {
            status: status.to_string(),
            score: breakdown.smoothed_score,
            raw_score: breakdown.overall_score,
            heartbeat: heartbeat.to_string(),
            timestamp: breakdown.timestamp.unix_timestamp(),
            breakdown: breakdown_struct,
            issues,
            resources,
        }
    }
}

/// Convert health score to status level
fn score_to_status(score: f64) -> &'static str {
    match score {
        s if s >= 90.0 => "excellent",
        s if s >= 75.0 => "good",
        s if s >= 50.0 => "fair",
        s if s >= 25.0 => "poor",
        _ => "critical",
    }
}

/// Convert health score to heartbeat color
fn score_to_heartbeat(score: f64) -> &'static str {
    match score {
        s if s >= 75.0 => "green",
        s if s >= 50.0 => "yellow",
        _ => "red",
    }
}
