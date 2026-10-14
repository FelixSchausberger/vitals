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
///
/// Each dimension carries two values: the raw penalty `R_j`, a normalized
/// 0..=`r_max` gauge used to label the dimension (the `*_status` fields), and
/// the weighted burden `alpha_j` × `R_j`, which is the quantity that shares
/// units with issue penalties. Only the weighted burdens sum to [`Self::total`].
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
    /// CPU burden: `alpha_cpu` × [`Self::cpu`].
    pub cpu_burden: f64,
    /// Memory burden: `alpha_memory` × [`Self::memory`].
    pub memory_burden: f64,
    /// Disk burden: `alpha_disk` × [`Self::disk`].
    pub disk_burden: f64,
    /// Load burden: `alpha_load` × [`Self::load`].
    pub load_burden: f64,
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

    // The bootstrap thresholds are per-core constants, so raw load is divided
    // by the core count before comparison: on a many-core machine raw load
    // alone would cross them while every core sits idle. The baseline branch
    // stays on raw load, because the sigmoid divides usage, mean and std by the
    // same scale and is therefore invariant to this normalization, and because
    // the baseline learns this machine's own load as reported.
    #[allow(clippy::cast_precision_loss)]
    let load_per_core = snapshot.load_average / snapshot.cpu_count.max(1) as f64;

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
            bootstrap_penalty(load_per_core, t.load_warning, t.load_error, config.r_max),
        )
    };

    let cpu_burden = config.alpha_cpu * cpu;
    let memory_burden = config.alpha_memory * memory;
    let disk_burden = config.alpha_disk * disk;
    let load_burden = config.alpha_load * load;
    let total = cpu_burden + memory_burden + disk_burden + load_burden;

    ResourcePenalties {
        cpu,
        memory,
        disk,
        load,
        cpu_burden,
        memory_burden,
        disk_burden,
        load_burden,
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
            cpu_count: 1,
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
            cpu_count: 1,
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
            cpu_count: 1,
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
            cpu_count: 1,
        };

        let p = compute_resource_penalties(&snap, &baseline, &config);
        assert_eq!(p.cpu, config.r_max * 0.9); // error tier
    }

    #[test]
    fn bootstrap_load_threshold_is_per_core() {
        let config = ResourceConfig::default();
        let baseline = ResourceBaseline::new(); // bootstrap: static thresholds

        // Load 8.0 on 8 cores is 1.0 per core, below the 2.0 warning threshold.
        let many_cores = ResourceSnapshot {
            cpu_usage: 10.0,
            memory_usage: 50.0,
            disk_usage: 40.0,
            load_average: 8.0,
            cpu_count: 8,
        };
        assert_eq!(
            compute_resource_penalties(&many_cores, &baseline, &config).load,
            0.0
        );

        // The same raw load on a single core is above the 5.0 error threshold.
        let one_core = ResourceSnapshot {
            cpu_count: 1,
            ..many_cores
        };
        assert_eq!(
            compute_resource_penalties(&one_core, &baseline, &config).load,
            config.r_max * 0.9
        );
    }

    #[test]
    fn burden_fields_are_weighted_and_sum_to_total() {
        let config = ResourceConfig::default();
        // Baseline far below the snapshot so every dimension scores non-zero.
        let baseline = baseline_with_cpu(10.0, 2.0, config.min_samples_for_baseline);
        let snap = ResourceSnapshot {
            cpu_usage: 90.0,
            memory_usage: 90.0,
            disk_usage: 90.0,
            load_average: 90.0,
            cpu_count: 1,
        };

        let p = compute_resource_penalties(&snap, &baseline, &config);
        // Each burden is its gauge scaled by the matching alpha weight.
        assert_eq!(p.cpu_burden, config.alpha_cpu * p.cpu);
        assert_eq!(p.memory_burden, config.alpha_memory * p.memory);
        assert_eq!(p.disk_burden, config.alpha_disk * p.disk);
        assert_eq!(p.load_burden, config.alpha_load * p.load);
        // And they are the only terms in the total, which is not the sum of the
        // unweighted gauges.
        let sum = p.cpu_burden + p.memory_burden + p.disk_burden + p.load_burden;
        assert!((sum - p.total).abs() < 1e-9, "burdens must sum to total");
    }
}
