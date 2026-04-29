//! Baseline-relative sigmoid resource penalty scoring.
//!
//! Instead of hardcoded thresholds (e.g., "80% CPU = warning"), TWHS compares
//! current resource usage against the *machine's own historical distribution*.
//!
//! ## Formula
//!
//! For each resource dimension j with current utilization u:
//!
//! ```text
//! R_j(u) = `r_max` × sigmoid((u - μ_j) / (σ_j × steepness))
//! ```
//!
//! Where `μ_j` and `σ_j` are the rolling mean and standard deviation from the
//! `ResourceBaseline` Welford estimator.
//!
//! **Key property:** A desktop at 85% CPU with baseline mean=80%±8% scores near
//! zero. The same utilization on a database server with baseline mean=12%±5%
//! scores near `r_max`. The penalty is *deviation from normal*, not absolute usage.
//!
//! ## Bootstrap
//!
//! When the baseline has fewer than `min_samples_for_baseline` samples, static
//! threshold-based penalties are used as a fallback. These produce a coarser
//! (step-function) penalty but are better than nothing.

use crate::{baseline::ResourceBaseline, config::ResourceConfig, types::ResourceSnapshot};

/// Computed resource penalties for each dimension.
#[derive(Debug, Clone)]
pub struct ResourcePenalties {
    /// CPU penalty before alpha weighting.
    pub cpu: f64,
    /// Memory penalty before alpha weighting.
    pub memory: f64,
    /// Disk penalty before alpha weighting.
    pub disk: f64,
    /// Load average penalty before alpha weighting.
    pub load: f64,
    /// Total weighted penalty: Σ `alpha_j` × `R_j`.
    pub total: f64,
    /// True if static thresholds are in use (baseline not yet learned).
    pub learning_mode: bool,
}

/// Compute resource penalties for a snapshot against a baseline.
#[must_use]
pub fn compute_resource_penalties(
    snapshot: &ResourceSnapshot,
    baseline: &ResourceBaseline,
    config: &ResourceConfig,
) -> ResourcePenalties {
    let has_baseline = baseline.sample_count >= config.min_samples_for_baseline;

    let (cpu, memory, disk, load) = if has_baseline {
        (
            sigmoid_penalty(
                snapshot.cpu_usage,
                baseline.cpu.mean,
                baseline.cpu.std(),
                config,
            ),
            sigmoid_penalty(
                snapshot.memory_usage,
                baseline.memory.mean,
                baseline.memory.std(),
                config,
            ),
            sigmoid_penalty(
                snapshot.disk_usage,
                baseline.disk.mean,
                baseline.disk.std(),
                config,
            ),
            sigmoid_penalty(
                snapshot.load_average,
                baseline.load.mean,
                baseline.load.std(),
                config,
            ),
        )
    } else {
        let t = &config.bootstrap;
        (
            bootstrap_penalty(snapshot.cpu_usage, t.cpu_warning, t.cpu_error, config.r_max),
            bootstrap_penalty(
                snapshot.memory_usage,
                t.memory_warning,
                t.memory_error,
                config.r_max,
            ),
            bootstrap_penalty(
                snapshot.disk_usage,
                t.disk_warning,
                t.disk_error,
                config.r_max,
            ),
            bootstrap_penalty(
                snapshot.load_average,
                t.load_warning,
                t.load_error,
                config.r_max,
            ),
        )
    };

    let total = config.alpha_cpu * cpu
        + config.alpha_memory * memory
        + config.alpha_disk * disk
        + config.alpha_load * load;

    ResourcePenalties {
        cpu,
        memory,
        disk,
        load,
        total,
        learning_mode: !has_baseline,
    }
}

/// Sigmoid-shaped penalty relative to baseline distribution.
///
/// Returns near 0 when usage is at or below the baseline mean,
/// and approaches `r_max` as usage deviates far above the mean.
fn sigmoid_penalty(usage: f64, mean: f64, std: f64, config: &ResourceConfig) -> f64 {
    // Avoid division by zero for degenerate baselines
    if std < 0.01 {
        return 0.0;
    }
    config.r_max * sigmoid((usage - mean) / (std * config.steepness))
}

/// Step-function penalty for bootstrap period (no baseline yet).
///
/// Two steps: warning (40% of `r_max`) and error (90% of `r_max`).
fn bootstrap_penalty(usage: f64, warn: f64, err: f64, r_max: f64) -> f64 {
    if usage >= err {
        r_max * 0.9
    } else if usage >= warn {
        r_max * 0.4
    } else {
        0.0
    }
}

fn sigmoid(x: f64) -> f64 {
    1.0 / (1.0 + (-x).exp())
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::float_cmp,
        clippy::uninlined_format_args,
        clippy::cast_precision_loss
    )]
    use super::*;
    use crate::{baseline::ResourceBaseline, config::ResourceConfig};

    fn baseline_with_cpu(mean: f64, std: f64, samples: u64) -> ResourceBaseline {
        // Build a Welford state that represents (mean, std) with given sample count.
        // We inject two synthetic samples: mean-std and mean+std.
        // This gives exact mean and approx std after enough samples.
        let mut baseline = ResourceBaseline::new();
        let half = samples / 2;
        for _ in 0..half {
            baseline.cpu.update(mean - std);
            baseline.cpu.update(mean + std);
            baseline.memory.update(50.0);
            baseline.disk.update(40.0);
            baseline.load.update(1.0);
        }
        baseline.sample_count = samples;
        baseline
    }

    #[test]
    fn normal_cpu_gives_low_penalty() {
        let config = ResourceConfig::default();
        // Baseline: mean=20%, std=10%
        let baseline = baseline_with_cpu(20.0, 10.0, config.min_samples_for_baseline);
        let snap = ResourceSnapshot {
            cpu_usage: 22.0, // just above mean — should be near zero penalty
            memory_usage: 50.0,
            disk_usage: 40.0,
            load_average: 1.0,
        };

        let p = compute_resource_penalties(&snap, &baseline, &config);
        assert!(!p.learning_mode, "should not be in learning mode");
        // 22% vs mean=20%, std=10%: deviation is 0.2σ — very small
        assert!(
            p.cpu < config.r_max * 0.6,
            "low deviation should give low penalty, got {}",
            p.cpu
        );
    }

    #[test]
    fn extreme_cpu_gives_high_penalty() {
        let config = ResourceConfig::default();
        // Baseline: mean=12%, std=5% (like a server at idle)
        let baseline = baseline_with_cpu(12.0, 5.0, config.min_samples_for_baseline);
        let snap = ResourceSnapshot {
            cpu_usage: 85.0, // way above baseline
            memory_usage: 50.0,
            disk_usage: 40.0,
            load_average: 1.0,
        };

        let p = compute_resource_penalties(&snap, &baseline, &config);
        assert!(!p.learning_mode);
        // (85 - 12) / (5 × 0.5) = 29.2 → sigmoid(29.2) ≈ 1.0 → r_max × 1.0 = 20.0
        assert!(
            p.cpu > config.r_max * 0.9,
            "extreme deviation should give near-max penalty, got {}",
            p.cpu
        );
    }

    #[test]
    fn bootstrap_mode_uses_static_thresholds() {
        let config = ResourceConfig::default();
        let baseline = ResourceBaseline::new(); // no samples
        let snap = ResourceSnapshot {
            // 90% is between warning (80%) and error (95%) → r_max × 0.4
            cpu_usage: 90.0,
            memory_usage: 50.0,
            disk_usage: 40.0,
            load_average: 1.0,
        };

        let p = compute_resource_penalties(&snap, &baseline, &config);
        assert!(
            p.learning_mode,
            "should be in learning mode with no samples"
        );
        assert_eq!(p.cpu, config.r_max * 0.4); // warning tier: 80% ≤ cpu < 95%
    }

    #[test]
    fn bootstrap_error_threshold() {
        let config = ResourceConfig::default();
        let baseline = ResourceBaseline::new();
        let snap = ResourceSnapshot {
            cpu_usage: 96.0, // above error threshold (95%)
            memory_usage: 50.0,
            disk_usage: 40.0,
            load_average: 1.0,
        };

        let p = compute_resource_penalties(&snap, &baseline, &config);
        assert_eq!(p.cpu, config.r_max * 0.9); // error tier
    }
}
