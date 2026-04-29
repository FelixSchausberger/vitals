//! Cascade attribution: reduce penalties for events caused by a parent failure.
//!
//! When service B fails because service A failed (a cascade), B's penalty should
//! be attenuated — it's a *consequence*, not an independent health signal.
//!
//! ## Attribution Strategy
//!
//! **Primary (systemd available):** If an event's `unit_dependencies` contains the
//! unit of a failing parent event, graph-confirmed attribution is applied at
//! `cascade_weight = 0.85`. B always retains ≥15% of its standalone penalty.
//!
//! **Fallback (no dependency graph):** Temporal co-onset inference. Events that
//! start failing within a tight window and share a "unit family" (same prefix
//! before `.` or `-`) are attributed using a weaker fraction (0.65).
//!
//! ## Non-Recursion
//! Attribution is single-pass only. If A → B → C, C is attributed to B, not to A.
//! This simplifies the implementation and avoids unbounded attribution chains.

use std::f64::consts::LN_2;

use crate::{config::CascadeConfig, types::Event};

/// Compute attribution fractions for each event (indexed to match input slice).
///
/// Returns a `Vec<f64>` where `result[i]` is the fraction of event `i`'s penalty
/// that can be attributed to a parent cascade. The caller applies:
///
/// ```text
/// adjusted_penalty[i] = raw_penalty[i] × (1.0 - attributions[i])
/// ```
#[must_use]
pub fn compute_attribution_fractions(events: &[Event], config: &CascadeConfig) -> Vec<f64> {
    let n = events.len();
    if n == 0 {
        return vec![];
    }

    let mu = LN_2 / config.half_life_secs;

    // Sort indices by first_seen (ascending) — earlier events are potential parents
    let mut sorted_idx: Vec<usize> = (0..n).collect();
    sorted_idx.sort_by(|&a, &b| events[a].first_seen.cmp(&events[b].first_seen));

    // For each event, accumulate the maximum attribution from any parent
    let mut attributions = vec![0.0f64; n];

    for pos_j in 1..sorted_idx.len() {
        let j = sorted_idx[pos_j];
        let child = &events[j];

        for &i in &sorted_idx[..pos_j] {
            let parent = &events[i];
            let attr = single_attribution(parent, child, mu, config);
            if attr > attributions[j] {
                attributions[j] = attr;
            }
        }
    }

    attributions
}

/// Compute the attribution fraction from a single (parent, child) pair.
///
/// Returns 0.0 if no attribution applies.
fn single_attribution(parent: &Event, child: &Event, mu: f64, config: &CascadeConfig) -> f64 {
    // --- Path 1: Explicit dependency graph ---
    // If child.unit_dependencies contains parent.unit, it's a confirmed cascade.
    if let (Some(child_unit), Some(parent_unit)) = (child.unit.as_deref(), parent.unit.as_deref()) {
        // Prevent self-attribution
        if child_unit != parent_unit
            && child
                .unit_dependencies
                .iter()
                .any(|dep| dep.as_str() == parent_unit)
        {
            return config.graph_weight;
        }
    }

    // --- Path 2: Temporal co-onset inference ---
    let onset_gap_secs = (child.first_seen - parent.first_seen)
        .as_seconds_f64()
        .max(0.0);
    let proximity = (-mu * onset_gap_secs).exp();

    if proximity < config.threshold {
        return 0.0;
    }

    // Require unit family similarity to reduce false positives
    if !same_unit_family(parent.unit.as_deref(), child.unit.as_deref()) {
        return 0.0;
    }

    proximity * config.temporal_weight
}

/// Two units share a "family" if they have the same prefix before the first `.` or `-`.
///
/// Examples:
/// - "nginx.service" and "nginx-proxy.service" → family "nginx" ✓
/// - "networkmanager.service" and "" → different families ✗
/// - Both None → no family → false (avoids spurious attribution for anonymous events)
fn same_unit_family(a: Option<&str>, b: Option<&str>) -> bool {
    match (a, b) {
        (Some(a), Some(b)) => unit_family(a) == unit_family(b),
        _ => false,
    }
}

fn unit_family(unit: &str) -> &str {
    unit.split(['.', '-']).next().unwrap_or(unit)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::float_cmp, clippy::uninlined_format_args)]
    use time::macros::datetime;

    use super::*;
    use crate::types::Severity;

    fn event(
        id: &str,
        unit: Option<&str>,
        deps: Vec<&str>,
        first_seen: time::OffsetDateTime,
    ) -> Event {
        Event {
            id: id.into(),
            severity: Severity::Error,
            title: id.into(),
            count: 1,
            first_seen,
            last_seen: first_seen,
            occurrence_timestamps_unix: vec![],
            unit: unit.map(Into::into),
            unit_dependencies: deps.into_iter().map(Into::into).collect(),
        }
    }

    #[test]
    fn graph_attribution_confirmed() {
        let t0 = datetime!(2024-01-01 12:00:00 UTC);
        let t1 = datetime!(2024-01-01 12:00:05 UTC);
        let config = CascadeConfig::default();

        // child explicitly depends on parent
        let parent = event("nm", Some("networkmanager.service"), vec![], t0);
        let child = event("wpa", Some(""), vec!["networkmanager.service"], t1);

        let attributions = compute_attribution_fractions(&[parent, child], &config);
        assert!(
            (attributions[1] - config.graph_weight).abs() < f64::EPSILON,
            "expected graph_weight={}, got {}",
            config.graph_weight,
            attributions[1]
        );
        assert_eq!(attributions[0], 0.0, "parent should not be attributed");
    }

    #[test]
    fn temporal_attribution_same_family() {
        let t0 = datetime!(2024-01-01 12:00:00 UTC);
        // 5 seconds later — well within 30s half-life
        let t1 = datetime!(2024-01-01 12:00:05 UTC);
        let config = CascadeConfig::default();

        // Same "nginx" family, close in time, no explicit deps
        let parent = event("p", Some("nginx.service"), vec![], t0);
        let child = event("c", Some("nginx-proxy.service"), vec![], t1);

        let attributions = compute_attribution_fractions(&[parent, child], &config);
        // Should be > 0 (temporal attribution applied)
        assert!(
            attributions[1] > 0.0,
            "expected temporal attribution, got 0"
        );
        // Should be ≤ temporal_weight
        assert!(
            attributions[1] <= config.temporal_weight,
            "attribution {} > temporal_weight {}",
            attributions[1],
            config.temporal_weight
        );
    }

    #[test]
    fn no_attribution_different_family() {
        let t0 = datetime!(2024-01-01 12:00:00 UTC);
        let t1 = datetime!(2024-01-01 12:00:03 UTC);
        let config = CascadeConfig::default();

        // Different families, even though close in time
        let a = event("a", Some("nginx.service"), vec![], t0);
        let b = event("b", Some("postgres.service"), vec![], t1);

        let attributions = compute_attribution_fractions(&[a, b], &config);
        assert_eq!(
            attributions[1], 0.0,
            "different families should not be attributed"
        );
    }

    #[test]
    fn no_attribution_far_apart_in_time() {
        let t0 = datetime!(2024-01-01 12:00:00 UTC);
        // 10 minutes later — far beyond 30s half-life
        let t1 = datetime!(2024-01-01 12:10:00 UTC);
        let config = CascadeConfig::default();

        let a = event("a", Some("nginx.service"), vec![], t0);
        let b = event("b", Some("nginx-proxy.service"), vec![], t1);

        let attributions = compute_attribution_fractions(&[a, b], &config);
        assert_eq!(
            attributions[1], 0.0,
            "events too far apart should not be attributed"
        );
    }

    #[test]
    fn parent_is_never_attributed() {
        let t0 = datetime!(2024-01-01 12:00:00 UTC);
        let t1 = datetime!(2024-01-01 12:00:05 UTC);
        let config = CascadeConfig::default();

        let parent = event("nm", Some("networkmanager.service"), vec![], t0);
        let child = event("wpa", Some(""), vec!["networkmanager.service"], t1);

        let attributions = compute_attribution_fractions(&[parent, child], &config);
        assert_eq!(attributions[0], 0.0, "parent should never be attributed");
    }
}
