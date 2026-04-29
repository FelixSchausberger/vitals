//! # scorer — Temporal Weighted Health Score
//!
//! A novel, transparent system health scoring algorithm that outputs a single
//! number in (0, 100] representing the health of a Linux system.
//!
//! ## Algorithm Overview
//!
//! ```text
//! score = 100 × exp(-T / κ)
//!
//! where T = Σ_i [w(sev_i) × F_i × (1 - cascade_i)] + Σ_j [α_j × R_j(u_j)]
//!
//! F_i = Σ_k exp(-λ · age_k)      (temporal frecency, λ = ln2 / half_life)
//! R_j = r_max × sigmoid((u - μ) / (σ × k))   (baseline-relative resource penalty)
//! ```
//!
//! ## Design Principles
//!
//! - **Time decay**: Recent issues weigh more than old ones. An issue from last week
//!   contributes near zero to today's score.
//! - **Cascade attribution**: If `NetworkManager` fails and 9 dependents fail, the 9
//!   downstream failures are attributed to `NetworkManager` and attenuated.
//! - **Baseline-relative resources**: A gaming desktop at 85% CPU scores differently
//!   than a database server at 85% — "normal" is machine-specific.
//! - **Non-linear normalization**: `100 × exp(-burden/κ)` never cliff-edges to 0.
//! - **Full transparency**: Every point of burden is traceable to a specific event.
//!
//! ## Quick Start
//!
//! ```rust
//! use scorer::{compute, TwhsConfig, Event, Severity, ResourceSnapshot, ResourceBaseline};
//! use time::OffsetDateTime;
//!
//! let events = vec![/* your collected events */];
//! let config = TwhsConfig::default();
//! let baseline = ResourceBaseline::new();
//! let now = OffsetDateTime::now_utc();
//!
//! let breakdown = compute(&events, None, Some(&baseline), &config, now);
//! println!("Health: {:.1}", breakdown.score);
//! ```

pub mod algorithm;
pub mod baseline;
pub mod config;
pub mod types;

// Convenience re-exports for the public API
use algorithm::{
    cascade::compute_attribution_fractions, frecency::temporal_frecency,
    normalization::burden_to_score, resources::compute_resource_penalties,
};
pub use baseline::ResourceBaseline;
pub use config::TwhsConfig;
use time::OffsetDateTime;
pub use types::{
    Event, EventContribution, HealthBreakdown, ResourceContribution, ResourceSnapshot,
    ScoringInput, Severity,
};

/// Compute the Temporal Weighted Health Score.
///
/// ## Arguments
///
/// - `events`: All currently active health events. Pass an empty slice for a clean system.
/// - `resource_snapshot`: Current resource utilization. `None` disables resource scoring.
/// - `baseline`: Learned resource baseline. `None` forces static threshold scoring.
/// - `config`: Algorithm parameters.
/// - `now`: Timestamp for age calculations. Pass `OffsetDateTime::now_utc()` in production.
///
/// ## Returns
///
/// A [`HealthBreakdown`] containing the score plus full transparency breakdown.
#[must_use]
pub fn compute(
    events: &[Event],
    resource_snapshot: Option<&ResourceSnapshot>,
    baseline: Option<&ResourceBaseline>,
    config: &TwhsConfig,
    now: OffsetDateTime,
) -> HealthBreakdown {
    // Step 1 & 2: Temporal frecency × severity weight → raw penalties
    // Frecency is capped per event so that a single crash-looping source cannot
    // produce unbounded burden. N repetitions of the same problem are still worse
    // than 1, but saturate rather than growing without bound.
    let frecencies: Vec<f64> = events
        .iter()
        .map(|e| {
            temporal_frecency(e, config.decay_half_life_hours, now)
                .min(config.max_frecency_per_event)
        })
        .collect();

    let raw_penalties: Vec<f64> = events
        .iter()
        .zip(&frecencies)
        .map(|(event, &frecency)| severity_weight(event.severity, config) * frecency)
        .collect();

    // Step 3: Cascade attribution
    let attributions = compute_attribution_fractions(events, &config.cascade);
    let adjusted_penalties: Vec<f64> = raw_penalties
        .iter()
        .zip(&attributions)
        .map(|(&raw, &attr)| raw * (1.0 - attr))
        .collect();

    // Step 4: Resource penalties
    let empty_baseline = ResourceBaseline::new();
    let resource_result = resource_snapshot.map(|snap| {
        let bl = baseline.unwrap_or(&empty_baseline);
        compute_resource_penalties(snap, bl, &config.resources)
    });

    // Step 5: Total burden
    let issue_burden: f64 = adjusted_penalties.iter().sum();
    let resource_burden = resource_result.as_ref().map_or(0.0, |r| r.total);
    let total_burden = issue_burden + resource_burden;

    // Step 6: Score mapping
    let score = burden_to_score(total_burden, config.sensitivity);

    // Step 7: Transparency — compute burden share percentages
    let mut contributions: Vec<EventContribution> = events
        .iter()
        .enumerate()
        .map(|(i, event)| EventContribution {
            id: event.id.clone(),
            title: event.title.clone(),
            severity: event.severity,
            temporal_frecency: frecencies[i],
            raw_penalty: raw_penalties[i],
            cascade_attribution_fraction: attributions[i],
            adjusted_penalty: adjusted_penalties[i],
            burden_share_pct: if total_burden > 1e-9 {
                (adjusted_penalties[i] / total_burden) * 100.0
            } else {
                0.0
            },
        })
        .collect();

    // Sort by adjusted penalty descending (worst offenders first)
    contributions.sort_by(|a, b| {
        b.adjusted_penalty
            .partial_cmp(&a.adjusted_penalty)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    let resource_contribution = resource_result.map(|r| ResourceContribution {
        cpu_penalty: r.cpu,
        memory_penalty: r.memory,
        disk_penalty: r.disk,
        load_penalty: r.load,
        total: r.total,
        burden_share_pct: if total_burden > 1e-9 {
            (r.total / total_burden) * 100.0
        } else {
            0.0
        },
        baseline_learning_mode: r.learning_mode,
    });

    let baseline_learning_mode =
        baseline.is_none_or(|b| b.sample_count < config.resources.min_samples_for_baseline);

    HealthBreakdown {
        score,
        total_burden,
        issue_burden,
        resource_burden,
        contributions,
        resource_contribution,
        baseline_learning_mode,
        observability_note: None,
    }
}

fn severity_weight(severity: Severity, config: &TwhsConfig) -> f64 {
    match severity {
        Severity::Error => config.error_weight,
        Severity::Warning => config.warning_weight,
        Severity::Info => config.info_weight,
    }
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::float_cmp,
        clippy::uninlined_format_args,
        clippy::cast_precision_loss
    )]
    use super::*;

    fn fresh_error(id: &str, unit: Option<&str>) -> Event {
        let now = OffsetDateTime::now_utc();
        Event {
            id: id.into(),
            severity: Severity::Error,
            title: format!("Test error: {}", id),
            count: 1,
            first_seen: now - time::Duration::minutes(1),
            last_seen: now - time::Duration::seconds(10),
            occurrence_timestamps_unix: vec![],
            unit: unit.map(Into::into),
            unit_dependencies: vec![],
        }
    }

    #[test]
    fn clean_system_scores_100() {
        let config = TwhsConfig::default();
        let now = OffsetDateTime::now_utc();
        let breakdown = compute(&[], None, None, &config, now);
        assert!(
            (breakdown.score - 100.0).abs() < f64::EPSILON,
            "empty events should score 100.0, got {}",
            breakdown.score
        );
        assert_eq!(breakdown.total_burden, 0.0);
        assert!(breakdown.contributions.is_empty());
    }

    #[test]
    fn single_error_reduces_score() {
        let config = TwhsConfig::default();
        let now = OffsetDateTime::now_utc();
        let events = vec![fresh_error("e1", None)];
        let breakdown = compute(&events, None, None, &config, now);
        assert!(
            breakdown.score < 100.0,
            "error should reduce score below 100"
        );
        assert!(breakdown.score > 0.0, "score should remain positive");
        assert_eq!(breakdown.contributions.len(), 1);
    }

    #[test]
    fn more_errors_reduce_score_further() {
        let config = TwhsConfig::default();
        let now = OffsetDateTime::now_utc();

        let one_error = compute(&[fresh_error("e1", None)], None, None, &config, now).score;
        let five_errors = compute(
            &[
                fresh_error("e1", None),
                fresh_error("e2", None),
                fresh_error("e3", None),
                fresh_error("e4", None),
                fresh_error("e5", None),
            ],
            None,
            None,
            &config,
            now,
        )
        .score;

        assert!(
            five_errors < one_error,
            "five errors should score lower than one: {} vs {}",
            five_errors,
            one_error
        );
    }

    #[test]
    fn cascade_reduces_child_penalty() {
        let config = TwhsConfig::default();
        let now = OffsetDateTime::now_utc();
        let t0 = now - time::Duration::minutes(5);
        let t1 = t0 + time::Duration::seconds(3);

        // Parent failure
        let parent = Event {
            id: "nm".into(),
            severity: Severity::Error,
            title: "NetworkManager failed".into(),
            count: 1,
            first_seen: t0,
            last_seen: t0,
            occurrence_timestamps_unix: vec![],
            unit: Some("networkmanager.service".into()),
            unit_dependencies: vec![],
        };

        // Child with explicit dependency on parent
        let child_with_dep = Event {
            id: "wpa".into(),
            severity: Severity::Error,
            title: "wpa_supplicant failed".into(),
            count: 1,
            first_seen: t1,
            last_seen: t1,
            occurrence_timestamps_unix: vec![],
            unit: Some("wpa_supplicant.service".into()),
            unit_dependencies: vec!["networkmanager.service".into()],
        };

        // Child without dependency (independent)
        let mut child_independent = child_with_dep.clone();
        child_independent.unit_dependencies = vec![];

        let with_cascade = compute(&[parent.clone(), child_with_dep], None, None, &config, now);
        let without_cascade = compute(&[parent, child_independent], None, None, &config, now);

        assert!(
            with_cascade.score > without_cascade.score,
            "cascade attribution should improve score: with={} without={}",
            with_cascade.score,
            without_cascade.score
        );
    }

    #[test]
    fn old_error_scores_better_than_recent() {
        let config = TwhsConfig::default();
        let now = OffsetDateTime::now_utc();

        // Recent error (1 minute ago)
        let recent = Event {
            id: "r".into(),
            severity: Severity::Error,
            title: "Recent error".into(),
            count: 1,
            first_seen: now - time::Duration::minutes(1),
            last_seen: now - time::Duration::minutes(1),
            occurrence_timestamps_unix: vec![],
            unit: None,
            unit_dependencies: vec![],
        };

        // Old error (3 days ago) — should have decayed significantly at 6h half-life
        let old = Event {
            id: "o".into(),
            severity: Severity::Error,
            title: "Old error".into(),
            count: 1,
            first_seen: now - time::Duration::days(3),
            last_seen: now - time::Duration::days(3),
            occurrence_timestamps_unix: vec![],
            unit: None,
            unit_dependencies: vec![],
        };

        let recent_score = compute(&[recent], None, None, &config, now).score;
        let old_score = compute(&[old], None, None, &config, now).score;

        assert!(
            old_score > recent_score,
            "old error should have decayed and score higher: old={} recent={}",
            old_score,
            recent_score
        );
    }

    #[test]
    fn crash_loop_capped_score_stays_reasonable() {
        // 325 warnings from a single crash-looping service must not crater the score.
        let config = TwhsConfig::default(); // max_frecency_per_event = 10.0
        let now = OffsetDateTime::now_utc();

        let crash_loop = Event {
            id: "journal-warn-4-user@1000.service".into(),
            severity: Severity::Warning,
            title: "WARN Journal Events".into(),
            count: 325,
            first_seen: now - time::Duration::minutes(30),
            last_seen: now - time::Duration::minutes(1),
            occurrence_timestamps_unix: vec![],
            unit: Some("user@1000.service".into()),
            unit_dependencies: vec![],
        };

        let breakdown = compute(&[crash_loop], None, None, &config, now);

        // Score must stay in a meaningful range — not near zero.
        // 325 warnings should produce "fair" or better (≥ 50), not "critical".
        assert!(
            breakdown.score >= 50.0,
            "crash-loop of 325 warnings should not crater score below 50, got {:.1}",
            breakdown.score
        );
        // Still penalised — not healthy
        assert!(
            breakdown.score < 100.0,
            "crash-loop should still reduce score below 100"
        );
    }

    #[test]
    fn burden_share_percentages_sum_to_100() {
        let config = TwhsConfig::default();
        let now = OffsetDateTime::now_utc();

        let events = vec![
            fresh_error("e1", None),
            fresh_error("e2", None),
            fresh_error("e3", None),
        ];

        let breakdown = compute(&events, None, None, &config, now);
        let total_pct: f64 = breakdown
            .contributions
            .iter()
            .map(|c| c.burden_share_pct)
            .sum();

        assert!(
            (total_pct - 100.0).abs() < 0.01,
            "burden shares should sum to 100%, got {}",
            total_pct
        );
    }
}
