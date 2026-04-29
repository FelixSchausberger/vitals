//! Issue model and severity definitions.
//!
//! Defines the core Issue structure that represents system problems
//! detected by aggregating journal entries and systemd unit states.

use std::fmt;

use serde::{Deserialize, Serialize};
use thiserror::Error;
use time::OffsetDateTime;

/// Severity levels for system issues.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Severity {
    /// Critical system failures requiring immediate attention
    Error = 0,
    /// Warnings that should be addressed but aren't critical
    Warning = 1,
    /// Informational issues for awareness
    Info = 2,
}

impl fmt::Display for Severity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Error => write!(f, "ERROR"),
            Self::Warning => write!(f, "WARN"),
            Self::Info => write!(f, "INFO"),
        }
    }
}

/// Trend analysis for issue frequency over time.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
pub enum IssueTrend {
    /// Issue frequency is increasing
    Increasing,
    /// Issue frequency is decreasing
    Decreasing,
    /// Issue frequency is stable
    #[default]
    Stable,
    /// Not enough data to determine trend
    Unknown,
}

impl fmt::Display for IssueTrend {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Increasing => write!(f, "↗ Increasing"),
            Self::Decreasing => write!(f, "↘ Decreasing"),
            Self::Stable => write!(f, "→ Stable"),
            Self::Unknown => write!(f, "? Unknown"),
        }
    }
}

/// Errors that can occur during issue processing.
#[derive(Error, Debug)]
#[allow(dead_code)]
pub enum IssueError {
    /// Invalid severity level provided
    #[error("Invalid severity level: {0}")]
    InvalidSeverity(String),

    /// Issue ID already exists
    #[error("Issue with ID '{0}' already exists")]
    DuplicateId(String),

    /// Invalid timestamp format
    #[error("Invalid timestamp format: {0}")]
    InvalidTimestamp(#[from] time::error::Parse),
}

/// A system issue detected from logs or systemd units.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Issue {
    /// Unique identifier for this issue
    pub id: String,
    /// Severity level of the issue
    pub severity: Severity,
    /// Human-readable title
    pub title: String,
    /// Detailed summary of the issue
    pub summary: String,
    /// Associated systemd unit if applicable
    pub unit: Option<String>,
    /// Process IDs related to this issue
    pub pids: Vec<u32>,
    /// Number of occurrences of this issue
    pub count: usize,
    /// When this issue was first observed
    #[serde(with = "time::serde::rfc3339")]
    pub first_seen: OffsetDateTime,
    /// When this issue was last observed
    #[serde(with = "time::serde::rfc3339")]
    pub last_seen: OffsetDateTime,
    /// Helpful hints for resolving the issue
    pub hints: Vec<String>,
    /// Frequency tracking - occurrences over time (timestamp, count at that time)
    #[serde(default)]
    pub occurrence_history: Vec<(OffsetDateTime, usize)>,
    /// Related journal entry IDs that contributed to this issue
    #[serde(default)]
    pub related_entries: Vec<String>,
    /// Issue trend - increasing, decreasing, stable
    #[serde(default)]
    pub trend: IssueTrend,
}

impl Issue {
    /// Create a new issue with the specified parameters.
    ///
    /// # Arguments
    /// * `id` - Unique identifier for the issue
    /// * `severity` - Severity level
    /// * `title` - Human-readable title
    /// * `summary` - Detailed description
    /// * `unit` - Associated systemd unit (optional)
    /// * `pids` - Related process IDs
    /// * `first_seen` - Timestamp when first observed
    /// * `last_seen` - Timestamp when last observed
    #[must_use]
    #[allow(clippy::too_many_arguments)]
    pub const fn new(
        id: String,
        severity: Severity,
        title: String,
        summary: String,
        unit: Option<String>,
        pids: Vec<u32>,
        first_seen: OffsetDateTime,
        last_seen: OffsetDateTime,
    ) -> Self {
        Self {
            id,
            severity,
            title,
            summary,
            unit,
            pids,
            count: 1,
            first_seen,
            last_seen,
            hints: Vec::new(),
            occurrence_history: Vec::new(),
            related_entries: Vec::new(),
            trend: IssueTrend::Unknown,
        }
    }

    /// Add helpful hints for resolving this issue.
    #[must_use]
    pub fn with_hints(mut self, hints: Vec<String>) -> Self {
        self.hints = hints;
        self
    }

    /// Increment the occurrence count for this issue and track the frequency.
    #[allow(dead_code)] // Used for dynamic issue tracking
    pub fn increment_count(&mut self) {
        self.count += 1;
        let now = OffsetDateTime::now_utc();
        self.last_seen = now;

        // Add to occurrence history
        self.occurrence_history.push((now, self.count));

        // Keep only last 100 occurrences to avoid unbounded growth
        if self.occurrence_history.len() > 100 {
            self.occurrence_history.remove(0);
        }

        // Update trend analysis
        self.update_trend();
    }

    /// Update the last seen timestamp with frequency tracking.
    #[allow(dead_code)]
    pub fn update_last_seen(&mut self, timestamp: OffsetDateTime) {
        self.last_seen = timestamp;
    }

    /// Add a related journal entry ID to this issue.
    #[allow(dead_code)]
    pub fn add_related_entry(&mut self, entry_id: String) {
        if !self.related_entries.contains(&entry_id) {
            self.related_entries.push(entry_id);
        }

        // Keep only last 50 related entries
        if self.related_entries.len() > 50 {
            self.related_entries.remove(0);
        }
    }

    /// Update trend analysis based on recent occurrence history.
    #[allow(dead_code)] // Part of dynamic issue tracking
    fn update_trend(&mut self) {
        if self.occurrence_history.len() < 3 {
            self.trend = IssueTrend::Unknown;
            return;
        }

        // Get recent points for trend analysis
        let recent_points = &self.occurrence_history[self.occurrence_history.len() - 3..];
        let first_count = recent_points[0].1;
        let last_count = recent_points[2].1;

        if last_count > first_count + 2 {
            self.trend = IssueTrend::Increasing;
        } else if first_count > last_count + 2 {
            self.trend = IssueTrend::Decreasing;
        } else {
            self.trend = IssueTrend::Stable;
        }
    }

    /// Get the frequency of occurrences in the last N minutes.
    #[must_use]
    pub fn frequency_in_last_minutes(&self, minutes: i64) -> usize {
        let cutoff = OffsetDateTime::now_utc() - time::Duration::minutes(minutes);
        self.occurrence_history
            .iter()
            .filter(|(timestamp, _)| *timestamp >= cutoff)
            .count()
    }

    /// Get occurrences per hour based on recent history.
    #[must_use]
    #[allow(dead_code, clippy::cast_precision_loss)]
    pub fn occurrences_per_hour(&self) -> f64 {
        if self.occurrence_history.len() < 2 {
            return 0.0;
        }

        let recent = &self.occurrence_history[self.occurrence_history.len().saturating_sub(10)..];
        if let (Some(first), Some(last)) = (recent.first(), recent.last()) {
            let time_diff = (last.0 - first.0).as_seconds_f64() / 3600.0; // Convert to hours
            if time_diff > 0.0 {
                let count_diff = last.1.saturating_sub(first.1) as f64;
                return count_diff / time_diff;
            }
        }

        0.0
    }

    /// Get the age of this issue (time since first seen).
    #[must_use]
    pub fn age(&self) -> time::Duration {
        OffsetDateTime::now_utc() - self.first_seen
    }

    /// Check if this issue is recent (within the last hour).
    #[must_use]
    pub fn is_recent(&self) -> bool {
        self.age() < time::Duration::hours(1)
    }

    /// Get a color associated with this issue's severity.
    #[must_use]
    #[allow(dead_code)]
    pub const fn severity_color(&self) -> &'static str {
        match self.severity {
            Severity::Error => "red",
            Severity::Warning => "yellow",
            Severity::Info => "blue",
        }
    }

    /// Suggest appropriate actions based on the issue content and context
    #[must_use]
    pub fn suggest_actions(&self) -> Vec<String> {
        let mut suggestions = Vec::new();

        // If there's an associated unit, suggest service management actions
        if let Some(unit) = &self.unit {
            // Analyze issue content to suggest appropriate actions
            let title_lower = self.title.to_lowercase();
            let summary_lower = self.summary.to_lowercase();

            // Service failed/crashed - suggest restart
            if title_lower.contains("failed")
                || title_lower.contains("crashed")
                || summary_lower.contains("failed")
                || summary_lower.contains("exit")
                || summary_lower.contains("terminated")
            {
                suggestions.push(format!("Restart {unit}"));
                suggestions.push(format!("Check {unit} status"));
            }

            // Configuration issues - suggest reload
            if title_lower.contains("config")
                || title_lower.contains("configuration")
                || summary_lower.contains("config")
                || summary_lower.contains("reload")
            {
                suggestions.push(format!("Reload {unit} configuration"));
            }

            // Permission issues - suggest checking service files
            if title_lower.contains("permission")
                || summary_lower.contains("permission")
                || title_lower.contains("access")
                || summary_lower.contains("access")
            {
                suggestions.push(format!("Check {unit} service file permissions"));
            }

            // High frequency issues - suggest more aggressive actions
            if self.count > 10 || self.frequency_in_last_minutes(60) > 5 {
                suggestions.push(format!("Stop and restart {unit} (high frequency issue)"));
                suggestions.push(format!("Investigate {unit} logs for root cause"));
            }

            // For error severity, always suggest service status check
            if self.severity == Severity::Error {
                suggestions.push(format!("Check detailed status of {unit}"));
            }
        }

        // Generic suggestions based on severity
        match self.severity {
            Severity::Error => {
                if suggestions.is_empty() {
                    suggestions.push("Investigate system logs".to_string());
                    suggestions.push("Check system resources".to_string());
                }
            }
            Severity::Warning => {
                suggestions.push("Monitor for escalation".to_string());
            }
            Severity::Info => {
                suggestions.push("Review for patterns".to_string());
            }
        }

        // Add dismiss suggestion for all issues
        suggestions.push("Dismiss this issue".to_string());

        suggestions
    }
}

impl fmt::Display for Issue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "[{}] {} (count: {}, unit: {})",
            self.severity,
            self.title,
            self.count,
            self.unit.as_deref().unwrap_or("none")
        )
    }
}
