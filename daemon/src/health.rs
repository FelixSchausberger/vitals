//! Scorer-based health scoring for vitals daemon.
//!
//! Wraps the `scorer` algorithm library, adapting daemon data types
//! to/from the algorithm's I/O types.

use std::collections::HashMap;

use scorer::{ResourceBaseline, ResourceSnapshot, TwhsConfig};
use time::OffsetDateTime;
// Re-export shared health types so daemon.rs can import them from this module
// without changing its import paths.
pub use vitals_core::health::{
    HealthBreakdown, IssueImpact, ResourceConsumer, ResourceHealthMetrics, ResourceStatus,
};

use crate::{
    data::traits::{Metrics, UnitMetrics},
    model::{Issue, Severity},
};

/// Scorer-based health calculator.
///
/// Replaces the old `HealthCalculator` (EWMA + linear penalties). This wrapper
/// converts daemon `Issue` slices into `scorer::Event`s, calls `scorer::compute()`,
/// and adapts the result to the shared `HealthBreakdown` type used by the HTTP API
/// and output formatters.
pub struct TwhsCalculator {
    config: TwhsConfig,
    baseline: ResourceBaseline,
}

impl TwhsCalculator {
    /// Create a new TWHS calculator.
    ///
    /// Pass a pre-loaded `ResourceBaseline` to resume from persisted state,
    /// or `ResourceBaseline::new()` to start fresh (bootstrap mode).
    #[must_use]
    pub fn new(config: TwhsConfig, baseline: ResourceBaseline) -> Self {
        Self { config, baseline }
    }

    /// Compute a health breakdown from issues and optional system metrics.
    ///
    /// Drop-in replacement for `HealthCalculator::update_with_metrics()`.
    /// The `unit_metrics` parameter is accepted for API compatibility but is
    /// not yet used by TWHS (unit resource trend probes are planned).
    pub fn compute(
        &mut self,
        issues: &[Issue],
        system_metrics: Option<&Metrics>,
        _unit_metrics: Option<&[UnitMetrics]>,
    ) -> HealthBreakdown {
        let now = OffsetDateTime::now_utc();

        // Count by severity and build a count lookup (EventContribution has no count field)
        let mut error_count = 0usize;
        let mut warning_count = 0usize;
        let mut info_count = 0usize;
        let mut count_by_id: HashMap<String, usize> = HashMap::with_capacity(issues.len());
        for issue in issues {
            match issue.severity {
                Severity::Error => error_count += 1,
                Severity::Warning => warning_count += 1,
                Severity::Info => info_count += 1,
            }
            count_by_id.insert(issue.id.clone(), issue.count);
        }

        // Convert Issues to scorer Events
        // unit_dependencies are empty here; D-Bus-based cascade attribution
        // is wired in a later integration step.
        let events: Vec<scorer::Event> = issues.iter().map(issue_to_event).collect();

        // Build a resource snapshot from system metrics if available
        let snapshot = system_metrics.map(metrics_to_snapshot);

        // Run the scorer algorithm
        let scorer_result = scorer::compute(
            &events,
            snapshot.as_ref(),
            Some(&self.baseline),
            &self.config,
            now,
        );

        // Update baseline (contamination-safe: skip samples when system is degraded)
        if let Some(ref snap) = snapshot {
            let score_ok =
                scorer_result.score >= self.config.resources.contamination_score_threshold;
            self.baseline.update(snap, score_ok);
        }

        // Adapt scorer contributions → IssueImpact
        let issue_impacts: Vec<IssueImpact> = scorer_result
            .contributions
            .iter()
            .map(|c| IssueImpact {
                id: c.id.clone(),
                title: c.title.clone(),
                severity: twhs_severity_to_core(c.severity),
                count: count_by_id.get(&c.id).copied().unwrap_or(1),
                impact: -c.adjusted_penalty,
            })
            .collect();

        // Adapt resource contribution → ResourceHealthMetrics
        let resource_metrics = snapshot.as_ref().and_then(|snap| {
            scorer_result
                .resource_contribution
                .as_ref()
                .map(|rc| ResourceHealthMetrics {
                    cpu_usage: snap.cpu_usage,
                    cpu_status: penalty_to_status(rc.cpu_penalty, self.config.resources.r_max),
                    memory_usage: snap.memory_usage,
                    memory_status: penalty_to_status(
                        rc.memory_penalty,
                        self.config.resources.r_max,
                    ),
                    disk_usage: snap.disk_usage,
                    disk_status: penalty_to_status(rc.disk_penalty, self.config.resources.r_max),
                    load_average: snap.load_average,
                    load_status: penalty_to_status(rc.load_penalty, self.config.resources.r_max),
                    resource_impact: -rc.total,
                    resource_hog_count: 0,
                    top_resource_consumers: vec![],
                })
        });

        HealthBreakdown {
            overall_score: scorer_result.score,
            smoothed_score: scorer_result.score,
            error_count,
            warning_count,
            info_count,
            total_issues: issues.len(),
            timestamp: now,
            issue_impacts,
            resource_metrics,
        }
    }

    /// Return a reference to the current resource baseline for persistence.
    #[must_use]
    pub fn baseline(&self) -> &ResourceBaseline {
        &self.baseline
    }
}

/// Convert a daemon `Issue` to a `scorer::Event`.
fn issue_to_event(issue: &Issue) -> scorer::Event {
    scorer::Event {
        id: issue.id.clone(),
        severity: core_severity_to_twhs(issue.severity),
        title: issue.title.clone(),
        count: issue.count,
        first_seen: issue.first_seen,
        last_seen: issue.last_seen,
        occurrence_timestamps_unix: issue
            .occurrence_history
            .iter()
            .map(|(ts, _)| ts.unix_timestamp())
            .collect(),
        unit: issue.unit.clone(),
        unit_dependencies: vec![],
    }
}

/// Convert daemon `Metrics` to a `scorer::ResourceSnapshot`.
fn metrics_to_snapshot(metrics: &Metrics) -> ResourceSnapshot {
    ResourceSnapshot {
        cpu_usage: metrics.cpu_usage,
        memory_usage: metrics.memory_usage,
        disk_usage: metrics.disk_usage,
        load_average: metrics.load_average,
    }
}

/// Map `scorer::Severity` to `vitals_core::issue::Severity`.
fn twhs_severity_to_core(s: scorer::Severity) -> Severity {
    match s {
        scorer::Severity::Error => Severity::Error,
        scorer::Severity::Warning => Severity::Warning,
        scorer::Severity::Info => Severity::Info,
    }
}

/// Map `vitals_core::issue::Severity` to `scorer::Severity`.
fn core_severity_to_twhs(s: Severity) -> scorer::Severity {
    match s {
        Severity::Error => scorer::Severity::Error,
        Severity::Warning => scorer::Severity::Warning,
        Severity::Info => scorer::Severity::Info,
    }
}

/// Derive a `ResourceStatus` from a TWHS penalty value.
///
/// Thresholds: < 20% of `r_max` → Healthy, < 60% → Warning, ≥ 60% → Critical.
fn penalty_to_status(penalty: f64, r_max: f64) -> ResourceStatus {
    if r_max <= 0.0 || penalty < r_max * 0.2 {
        ResourceStatus::Healthy
    } else if penalty < r_max * 0.6 {
        ResourceStatus::Warning
    } else {
        ResourceStatus::Critical
    }
}

/// Convert a health score to a human-readable status label.
#[must_use]
pub fn score_to_status(score: f64) -> &'static str {
    match score {
        s if s >= 90.0 => "excellent",
        s if s >= 75.0 => "good",
        s if s >= 50.0 => "fair",
        s if s >= 25.0 => "poor",
        _ => "critical",
    }
}

/// Convert a health score to a heartbeat color for display.
#[must_use]
pub fn score_to_heartbeat_color(score: f64) -> &'static str {
    match score {
        s if s >= 75.0 => "green",
        s if s >= 50.0 => "yellow",
        _ => "red",
    }
}
