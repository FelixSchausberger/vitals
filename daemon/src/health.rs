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
/// Scorer-based health calculator using TWHS (Temporal Weighted Health Score).
///
/// This wrapper
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
    /// Per-unit metrics feed the resource-hog counter and the top-consumer
    /// list; scoring itself stays baseline-relative and system-wide.
    pub fn compute(
        &mut self,
        issues: &[Issue],
        system_metrics: Option<&Metrics>,
        unit_metrics: Option<&[UnitMetrics]>,
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
            .map(|c| {
                let original = issues.iter().find(|i| i.id == c.id);
                IssueImpact {
                    id: c.id.clone(),
                    title: c.title.clone(),
                    severity: twhs_severity_to_core(c.severity),
                    count: count_by_id.get(&c.id).copied().unwrap_or(1),
                    impact: -c.adjusted_penalty,
                    unit: original.and_then(|i| i.unit.clone()),
                    hints: original.map_or(vec![], |i| i.hints.clone()),
                }
            })
            .collect();

        // Adapt resource contribution → ResourceHealthMetrics
        let (resource_hog_count, top_resource_consumers) = top_consumers(unit_metrics);
        let resource_metrics = snapshot.as_ref().and_then(|snap| {
            scorer_result
                .resource_contribution
                .as_ref()
                .map(|rc| ResourceHealthMetrics {
                    cpu_usage: snap.cpu_usage,
                    cpu_penalty: rc.cpu_penalty,
                    cpu_burden: rc.cpu_burden,
                    cpu_status: penalty_to_status(rc.cpu_penalty, self.config.resources.r_max),
                    memory_usage: snap.memory_usage,
                    memory_penalty: rc.memory_penalty,
                    memory_burden: rc.memory_burden,
                    memory_status: penalty_to_status(
                        rc.memory_penalty,
                        self.config.resources.r_max,
                    ),
                    disk_usage: snap.disk_usage,
                    disk_penalty: rc.disk_penalty,
                    disk_burden: rc.disk_burden,
                    disk_status: penalty_to_status(rc.disk_penalty, self.config.resources.r_max),
                    load_average: snap.load_average,
                    load_penalty: rc.load_penalty,
                    load_burden: rc.load_burden,
                    load_status: penalty_to_status(rc.load_penalty, self.config.resources.r_max),
                    resource_impact: -rc.total,
                    resource_hog_count,
                    top_resource_consumers,
                    baseline_learning_mode: scorer_result.baseline_learning_mode,
                    baseline_samples: self.baseline.sample_count,
                    min_samples_for_baseline: self.config.resources.min_samples_for_baseline,
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
        // The scorer compares load per core against its per-core bootstrap
        // thresholds, so the snapshot must carry the core count.
        cpu_count: num_cpus::get(),
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

/// Per-unit consumers reported in the API and TUI.
const TOP_CONSUMER_LIMIT: usize = 5;
/// A unit at or above either threshold counts as a resource hog.
const RESOURCE_HOG_CPU_PERCENT: f64 = 50.0;
/// Memory threshold for the resource-hog counter, in MiB.
const RESOURCE_HOG_MEMORY_MB: u64 = 1024;
/// Units below both minimums are noise (and numerous) on a busy host.
const TOP_CONSUMER_MIN_CPU: f64 = 1.0;
/// CPU-independent minimum memory for the top-consumer list, in MiB.
const TOP_CONSUMER_MIN_MEMORY_MB: u64 = 100;

/// Rank per-unit consumers and count the units above a hog threshold.
///
/// `unit_metrics` is `None` when unit collection is disabled; the report then
/// carries zero hogs and an empty list rather than inventing data.
#[allow(clippy::cast_precision_loss)]
fn top_consumers(unit_metrics: Option<&[UnitMetrics]>) -> (usize, Vec<ResourceConsumer>) {
    let Some(units) = unit_metrics else {
        return (0, Vec::new());
    };

    let mut hog_count = 0;
    let mut consumers = Vec::new();

    for unit in units {
        let memory_mb = unit.memory_rss / (1024 * 1024);
        if unit.cpu_usage >= RESOURCE_HOG_CPU_PERCENT || memory_mb >= RESOURCE_HOG_MEMORY_MB {
            hog_count += 1;
        }
        if unit.cpu_usage < TOP_CONSUMER_MIN_CPU && memory_mb < TOP_CONSUMER_MIN_MEMORY_MB {
            continue;
        }

        consumers.push(ResourceConsumer {
            unit_name: unit.unit_name.clone(),
            cpu_usage: unit.cpu_usage,
            memory_mb,
            // Ranking only: one percent of CPU weighs the same as 100 MiB,
            // so a 1 GiB service outranks a 10 % one.
            impact_score: unit.cpu_usage + memory_mb as f64 / 100.0,
        });
    }

    consumers.sort_by(|a, b| b.impact_score.total_cmp(&a.impact_score));
    consumers.truncate(TOP_CONSUMER_LIMIT);
    (hog_count, consumers)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unit(name: &str, cpu: f64, memory_mb: u64) -> UnitMetrics {
        UnitMetrics {
            unit_name: name.to_string(),
            cpu_usage: cpu,
            memory_rss: memory_mb * 1024 * 1024,
            memory_vsz: 0,
            io_read_bps: 0,
            io_write_bps: 0,
            net_rx_bps: 0,
            net_tx_bps: 0,
            pids: vec![],
        }
    }

    #[test]
    fn no_unit_metrics_means_no_consumers() {
        let (hog_count, consumers) = top_consumers(None);
        assert_eq!(hog_count, 0);
        assert!(consumers.is_empty());
    }

    #[test]
    fn hogs_and_ranking_cover_cpu_and_memory() {
        let units = [
            unit("cpu.service", 80.0, 200),
            unit("memory.service", 5.0, 2048),
            unit("idle.service", 0.5, 10),
        ];

        let (hog_count, consumers) = top_consumers(Some(&units));

        assert_eq!(hog_count, 2, "80 % CPU and 2 GiB are both hogs");
        assert_eq!(consumers.len(), 2, "the idle unit is filtered out");
        assert_eq!(consumers[0].unit_name, "cpu.service");
        assert_eq!(consumers[1].unit_name, "memory.service");
        assert_eq!(consumers[1].memory_mb, 2048);
    }

    #[test]
    fn consumer_list_is_capped() {
        let units: Vec<UnitMetrics> = (0..8)
            .map(|i| unit(&format!("u{i}.service"), 10.0 + f64::from(i), 200))
            .collect();

        let (_, consumers) = top_consumers(Some(&units));

        assert_eq!(consumers.len(), TOP_CONSUMER_LIMIT);
        assert_eq!(consumers[0].unit_name, "u7.service");
    }
}
