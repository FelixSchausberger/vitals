//! Temporal frecency scoring.
//!
//! Computes `F_i` = `Σ_k` exp(-λ · `Δt_k`) for each event, where `Δt_k` is the age in
//! seconds of the k-th occurrence. λ = ln(2) / (`half_life_hours` × 3600).
//!
//! This is the continuous-time generalization of zoxide's discrete frecency
//! multipliers (×4 within 1h, ×2 within 1d, ÷2 within 1w, ÷4 otherwise).
//! The exponential gives smooth, principled decay with no step-function jumps.

use std::f64::consts::LN_2;

use time::OffsetDateTime;

use crate::types::Event;

/// Compute the temporal frecency score for a single event.
///
/// Uses per-occurrence timestamps when available. Falls back to a closed-form
/// approximation assuming uniform distribution over [`first_seen`, `last_seen`].
#[must_use]
pub fn temporal_frecency(event: &Event, half_life_hours: f64, now: OffsetDateTime) -> f64 {
    let lambda = LN_2 / (half_life_hours * 3600.0);
    let timestamps = event.occurrence_datetimes();

    if timestamps.is_empty() {
        fallback_frecency(event.count, event.first_seen, event.last_seen, lambda, now)
    } else {
        exact_frecency(&timestamps, lambda, now)
    }
}

/// Exact frecency: sum of exponential contributions from each timestamp.
fn exact_frecency(timestamps: &[OffsetDateTime], lambda: f64, now: OffsetDateTime) -> f64 {
    timestamps
        .iter()
        .map(|&ts| {
            let age_secs = (now - ts).as_seconds_f64().max(0.0);
            (-lambda * age_secs).exp()
        })
        .sum()
}

/// Closed-form fallback when per-occurrence timestamps are unavailable.
///
/// Treats `count` events as uniformly distributed over [`first_seen`, `last_seen`]:
///
/// F = count × (1 / (λ·span)) × (exp(-λ·age_last) - exp(-λ·(age_last + span)))
///
/// This is the closed-form integral of the uniform distribution convolved with
/// the exponential decay kernel.
#[allow(clippy::cast_precision_loss)]
fn fallback_frecency(
    count: usize,
    first_seen: OffsetDateTime,
    last_seen: OffsetDateTime,
    lambda: f64,
    now: OffsetDateTime,
) -> f64 {
    if count == 0 {
        return 0.0;
    }

    let age_last = (now - last_seen).as_seconds_f64().max(0.0);
    let span = (last_seen - first_seen).as_seconds_f64().max(0.0);

    if span < 1.0 || count == 1 {
        // All events at last_seen
        return count as f64 * (-lambda * age_last).exp();
    }

    // Closed-form integral: ∫_{age_last}^{age_last+span} exp(-λt) dt / span × count
    let exp_recent = (-lambda * age_last).exp();
    let exp_oldest = (-lambda * (age_last + span)).exp();
    count as f64 * (exp_recent - exp_oldest) / (lambda * span)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::float_cmp, clippy::uninlined_format_args)]
    #![allow(clippy::float_cmp, clippy::uninlined_format_args)]
    use time::macros::datetime;

    use super::*;
    use crate::types::{Event, Severity};

    fn make_event(count: usize, first_seen: OffsetDateTime, last_seen: OffsetDateTime) -> Event {
        Event {
            id: "test".into(),
            severity: Severity::Error,
            title: "Test".into(),
            count,
            first_seen,
            last_seen,
            occurrence_timestamps_unix: vec![],
            unit: None,
            unit_dependencies: vec![],
        }
    }

    #[test]
    fn zero_count_gives_zero() {
        let now = datetime!(2024-01-01 12:00:00 UTC);
        let event = make_event(0, now, now);
        assert_eq!(temporal_frecency(&event, 6.0, now), 0.0);
    }

    #[test]
    fn fresh_single_event_scores_near_one() {
        let now = datetime!(2024-01-01 12:00:00 UTC);
        // Event 1 second ago
        let ts = now - time::Duration::seconds(1);
        let event = make_event(1, ts, ts);
        let score = temporal_frecency(&event, 6.0, now);
        // exp(-λ × 1s) ≈ 1.0 for 6h half-life
        assert!((score - 1.0).abs() < 0.01, "score={}", score);
    }

    #[test]
    fn old_event_scores_near_zero() {
        let now = datetime!(2024-01-01 12:00:00 UTC);
        // Event 7 days ago — at 6h half-life, exp(-28 × ln2) ≈ 3.7e-9
        let ts = now - time::Duration::days(7);
        let event = make_event(1, ts, ts);
        let score = temporal_frecency(&event, 6.0, now);
        assert!(score < 1e-6, "score={}", score);
    }

    #[test]
    fn multiple_recent_events_accumulate() {
        let now = datetime!(2024-01-01 12:00:00 UTC);
        let ts = now - time::Duration::minutes(5);
        let event = make_event(10, ts, ts);
        let score = temporal_frecency(&event, 6.0, now);
        // 10 events 5 min old, each ≈ 1.0
        assert!(score > 9.0 && score <= 10.0, "score={}", score);
    }

    #[test]
    fn exact_timestamps_match_fallback_for_uniform_distribution() {
        let now = datetime!(2024-01-01 12:00:00 UTC);
        let first = now - time::Duration::hours(2);
        let last = now - time::Duration::hours(1);

        // Exact: 5 timestamps uniformly spread over [first, last]
        let span_secs = (last - first).whole_seconds();
        let timestamps: Vec<i64> = (0..5)
            .map(|i| (first + time::Duration::seconds(span_secs * i / 4)).unix_timestamp())
            .collect();

        let mut event_exact = make_event(5, first, last);
        event_exact.occurrence_timestamps_unix = timestamps;

        let event_fallback = make_event(5, first, last);

        let exact = temporal_frecency(&event_exact, 6.0, now);
        let approx = temporal_frecency(&event_fallback, 6.0, now);

        // Should be within 15% of each other
        let ratio = (exact - approx).abs() / exact.max(approx);
        assert!(
            ratio < 0.15,
            "exact={} approx={} ratio={}",
            exact,
            approx,
            ratio
        );
    }
}
