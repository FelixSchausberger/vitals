//! Configuration for the TWHS algorithm.

use serde::{Deserialize, Serialize};

/// Top-level configuration for the Temporal Weighted Health Score algorithm.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TwhsConfig {
    /// Half-life of event relevance in hours.
    ///
    /// Controls how quickly past events fade from the score:
    /// - 1h  — aggressive (fast healing, good for frequently restarting services)
    /// - 6h  — balanced (default)
    /// - 24h — conservative (low-change production systems)
    pub decay_half_life_hours: f64,

    /// Base penalty weight for Error-severity events. Default: 10.0
    pub error_weight: f64,

    /// Base penalty weight for Warning-severity events.
    /// Default: sqrt(10) ≈ 3.162 — one geometric step below `error_weight`.
    pub warning_weight: f64,

    /// Base penalty weight for Info-severity events. Default: 1.0
    pub info_weight: f64,

    /// Cascade attribution configuration.
    pub cascade: CascadeConfig,

    /// Sensitivity constant κ for score mapping: score = 100 × exp(-burden / κ).
    ///
    /// κ is the total burden at which the score reaches ~36.8 (100/e).
    /// Calibrated so that one fresh Error (count=1, ~5 min old) produces score ≈ 90,
    /// and five independent fresh Errors produce score ≈ 60.
    /// Default: 100.0
    pub sensitivity: f64,

    /// Maximum temporal frecency a single event can contribute to the burden.
    ///
    /// Prevents a single crash-looping source from producing unbounded burden.
    /// Without this cap, N occurrences of the same event contribute N × weight,
    /// collapsing the score to near-zero even when only one service is misbehaving.
    ///
    /// The cap bounds the worst-case burden per event at `max_frecency × severity_weight`:
    /// - An error event (weight 10) tops out at 10 × 10 = 100 burden → score ≈ 37
    /// - A warning event (weight 3.16) tops out at 10 × 3.16 = 31.6 burden → score ≈ 73
    ///
    /// Set to `f64::INFINITY` to disable. Default: 10.0
    pub max_frecency_per_event: f64,

    /// Resource penalty configuration.
    pub resources: ResourceConfig,
}

impl Default for TwhsConfig {
    fn default() -> Self {
        Self {
            decay_half_life_hours: 6.0,
            error_weight: 10.0,
            warning_weight: 10.0_f64.sqrt(), // ≈ 3.162: geometric scale
            info_weight: 1.0,
            cascade: CascadeConfig::default(),
            sensitivity: 100.0,
            max_frecency_per_event: 10.0,
            resources: ResourceConfig::default(),
        }
    }
}

/// Configuration for cascade attribution between related events.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CascadeConfig {
    /// Half-life (in seconds) of temporal proximity between events.
    ///
    /// Two events that start failing within this window are cascade candidates.
    /// Default: 30s — services that cascade typically do so within seconds.
    pub half_life_secs: f64,

    /// Minimum temporal proximity to consider an event as a cascade child.
    /// Must be in (0.0, 1.0). Default: 0.5
    pub threshold: f64,

    /// Maximum fraction of a child's penalty attributed away when a confirmed
    /// graph dependency is found. The child always retains at least 1 - `graph_weight`
    /// of its penalty (never fully zeroed). Default: 0.85
    pub graph_weight: f64,

    /// Maximum attribution fraction for temporally-inferred cascades
    /// (weaker than explicit graph edges). Default: 0.65
    pub temporal_weight: f64,
}

impl Default for CascadeConfig {
    fn default() -> Self {
        Self {
            half_life_secs: 30.0,
            threshold: 0.5,
            graph_weight: 0.85,
            temporal_weight: 0.65,
        }
    }
}

/// Configuration for resource penalty scoring.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourceConfig {
    /// Maximum penalty contribution from a single resource dimension. Default: 20.0
    pub r_max: f64,

    /// Steepness of the sigmoid function. Lower = more gradual transition. Default: 0.5
    pub steepness: f64,

    /// Weight of CPU in total resource penalty. Default: 0.35
    pub alpha_cpu: f64,

    /// Weight of memory in total resource penalty. Default: 0.30
    pub alpha_memory: f64,

    /// Weight of disk in total resource penalty. Default: 0.20
    pub alpha_disk: f64,

    /// Weight of load average in total resource penalty. Default: 0.15
    pub alpha_load: f64,

    /// Number of resource samples required before baseline is considered reliable.
    ///
    /// At 1s polling: 300 samples = 5 minutes of data.
    /// Until this threshold, bootstrap thresholds are used. Default: 300
    pub min_samples_for_baseline: u64,

    /// Don't update the baseline with samples collected when the score is below
    /// this threshold. Prevents a degraded state from contaminating the baseline.
    /// Default: 80.0
    pub contamination_score_threshold: f64,

    /// Static fallback thresholds used during the bootstrap period.
    pub bootstrap: BootstrapThresholds,
}

impl Default for ResourceConfig {
    fn default() -> Self {
        Self {
            r_max: 20.0,
            steepness: 0.5,
            alpha_cpu: 0.35,
            alpha_memory: 0.30,
            alpha_disk: 0.20,
            alpha_load: 0.15,
            min_samples_for_baseline: 300,
            contamination_score_threshold: 80.0,
            bootstrap: BootstrapThresholds::default(),
        }
    }
}

/// Static thresholds used for resource scoring during the baseline bootstrap period.
///
/// These are deliberately conservative (generic) defaults. For better bootstrap accuracy,
/// use [`crate::baseline::ResourceBaseline::from_system_profile`] to seed the baseline
/// from hardware classification.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BootstrapThresholds {
    pub cpu_warning: f64,
    pub cpu_error: f64,
    pub memory_warning: f64,
    pub memory_error: f64,
    pub disk_warning: f64,
    pub disk_error: f64,
    pub load_warning: f64,
    pub load_error: f64,
}

impl Default for BootstrapThresholds {
    fn default() -> Self {
        Self {
            cpu_warning: 80.0,
            cpu_error: 95.0,
            memory_warning: 85.0,
            memory_error: 95.0,
            disk_warning: 85.0,
            disk_error: 95.0,
            load_warning: 2.0, // per core
            load_error: 5.0,   // per core
        }
    }
}
