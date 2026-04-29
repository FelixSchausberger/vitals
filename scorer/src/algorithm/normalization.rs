//! Score normalization: map total burden to a health score in (0, 100].
//!
//! ## Formula
//!
//! ```text
//! score = 100 × exp(-burden / κ)
//! ```
//!
//! Where κ (sensitivity) is the burden value at which the score reaches 100/e ≈ 36.8.
//!
//! ## Why Negative Exponential?
//!
//! Compared to the current linear approach (`max(0, 100 - burden)`):
//!
//! | Property | Linear | Negative Exponential |
//! |---|---|---|
//! | Score at burden=0 | 100 ✓ | 100 ✓ |
//! | Score range | [0, 100] (clamped) | (0, 100] (natural) |
//! | Cliff edge at burden=100 | Yes ✗ | No ✓ |
//! | Behaviour as burden→∞ | Stuck at 0 | Approaches 0 smoothly ✓ |
//! | Convex (sensitive near healthy range) | No | Yes ✓ |
//!
//! The convex shape means the score degrades meaningfully when the first issue
//! appears (high sensitivity in the healthy range) and then decays more slowly
//! for already-degraded systems (graceful collapse).

/// Map total burden to a health score in (0.0, 100.0].
///
/// - `burden = 0.0`   → score = 100.0
/// - `burden = κ`     → score ≈ 36.8
/// - `burden → ∞`    → score → 0.0 (never exactly 0)
#[must_use]
pub fn burden_to_score(burden: f64, sensitivity: f64) -> f64 {
    debug_assert!(sensitivity > 0.0, "sensitivity must be positive");
    100.0 * (-burden / sensitivity).exp()
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::float_cmp,
        clippy::uninlined_format_args,
        clippy::cast_precision_loss
    )]
    use super::*;

    #[test]
    fn zero_burden_is_perfect_health() {
        let score = burden_to_score(0.0, 300.0);
        assert!((score - 100.0).abs() < f64::EPSILON);
    }

    #[test]
    fn burden_equal_sensitivity_gives_inverse_e() {
        let kappa = 300.0;
        let score = burden_to_score(kappa, kappa);
        let expected = 100.0 / std::f64::consts::E;
        assert!(
            (score - expected).abs() < 1e-10,
            "score={} expected={}",
            score,
            expected
        );
    }

    #[test]
    fn score_is_always_positive_in_practical_range() {
        // For astronomically large burdens, f64 exp() underflows to 0 — this is
        // a floating-point implementation detail, not a design flaw. In practice
        // burdens above ~10× κ represent completely broken systems.
        for burden in [0.0, 100.0, 500.0, 1000.0, 3000.0] {
            let score = burden_to_score(burden, 300.0);
            assert!(
                score > 0.0,
                "score should never reach 0, got {} for burden={}",
                score,
                burden
            );
        }
    }

    #[test]
    fn score_is_monotonically_decreasing_with_burden() {
        let kappa = 300.0;
        let burdens = [0.0, 50.0, 150.0, 300.0, 600.0, 1200.0];
        let scores: Vec<f64> = burdens.iter().map(|&b| burden_to_score(b, kappa)).collect();
        for window in scores.windows(2) {
            assert!(
                window[0] > window[1],
                "score should decrease as burden increases"
            );
        }
    }
}
