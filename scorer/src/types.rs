//! Input and output types for the TWHS algorithm.

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

/// Severity of a system health event.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Severity {
    /// Critical failures requiring immediate attention.
    Error = 0,
    /// Conditions that should be addressed but aren't immediately critical.
    Warning = 1,
    /// Informational observations.
    Info = 2,
}

impl std::fmt::Display for Severity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Error => write!(f, "error"),
            Self::Warning => write!(f, "warning"),
            Self::Info => write!(f, "info"),
        }
    }
}

/// A health-relevant system event, fed into the TWHS algorithm.
///
/// Events come from any source: systemd journal, failed units, active probes,
/// custom monitors. The algorithm is I/O-free; callers are responsible for
/// collecting events and converting them to this type.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Event {
    /// Unique identifier, stable across scoring calls for the same logical issue.
    pub id: String,

    /// Severity level.
    pub severity: Severity,

    /// Human-readable description.
    pub title: String,

    /// Number of occurrences (used for fallback frecency when timestamps are sparse).
    pub count: usize,

    /// When this event was first observed.
    #[serde(with = "time::serde::rfc3339")]
    pub first_seen: OffsetDateTime,

    /// When this event was last observed.
    #[serde(with = "time::serde::rfc3339")]
    pub last_seen: OffsetDateTime,

    /// Per-occurrence Unix timestamps (seconds). Used for precise temporal frecency.
    ///
    /// When empty, the algorithm falls back to a uniform-distribution approximation
    /// over [`first_seen`, `last_seen`] × count.
    #[serde(default)]
    pub occurrence_timestamps_unix: Vec<i64>,

    /// Systemd unit name (or equivalent), if applicable.
    #[serde(default)]
    pub unit: Option<String>,

    /// Units that this event's unit explicitly depends on (from systemd `Requires`/`BindsTo`).
    ///
    /// When populated, cascade attribution uses the explicit graph instead of temporal inference.
    #[serde(default)]
    pub unit_dependencies: Vec<String>,
}

impl Event {
    /// Convert `occurrence_timestamps_unix` to `OffsetDateTime` values.
    #[must_use]
    pub fn occurrence_datetimes(&self) -> Vec<OffsetDateTime> {
        self.occurrence_timestamps_unix
            .iter()
            .filter_map(|&ts| OffsetDateTime::from_unix_timestamp(ts).ok())
            .collect()
    }
}

/// A point-in-time snapshot of system resource utilization.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourceSnapshot {
    /// CPU usage as a percentage (0–100).
    pub cpu_usage: f64,
    /// Memory usage as a percentage (0–100).
    pub memory_usage: f64,
    /// Disk usage as a percentage (0–100).
    pub disk_usage: f64,
    /// System load average (1-minute, raw — not normalized by CPU count).
    pub load_average: f64,
}

/// The full input payload for a single scoring invocation.
///
/// Designed for JSON pipe usage: `collect-events | twhs`
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScoringInput {
    /// System health events to score.
    pub events: Vec<Event>,

    /// Current resource utilization, if available.
    #[serde(default)]
    pub resource_snapshot: Option<ResourceSnapshot>,
}

/// The complete result of a TWHS scoring call.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthBreakdown {
    /// The health score in (0.0, 100.0].
    ///
    /// Mapped via: score = 100 × exp(-burden / κ)
    /// - 100.0 = no issues detected
    /// - ~36.8 = burden equals sensitivity constant κ
    /// - approaches 0.0 as burden → ∞ (never exactly 0)
    pub score: f64,

    /// Total burden driving the score: `T` = `issue_burden` + `resource_burden`
    pub total_burden: f64,

    /// Sum of adjusted event penalties.
    pub issue_burden: f64,

    /// Total resource penalty contribution.
    pub resource_burden: f64,

    /// Per-event transparency breakdown, sorted by `adjusted_penalty` descending.
    pub contributions: Vec<EventContribution>,

    /// Resource penalty breakdown, if resource data was provided.
    #[serde(default)]
    pub resource_contribution: Option<ResourceContribution>,

    /// True during the baseline learning period (first N samples).
    /// Resource scoring uses static bootstrap thresholds during this phase.
    pub baseline_learning_mode: bool,

    /// Human-readable note when observability is limited.
    #[serde(default)]
    pub observability_note: Option<String>,
}

impl HealthBreakdown {
    /// Map the score to a human-readable status label.
    #[must_use]
    pub fn status(&self) -> &'static str {
        match self.score {
            s if s >= 90.0 => "excellent",
            s if s >= 75.0 => "good",
            s if s >= 50.0 => "fair",
            s if s >= 25.0 => "poor",
            _ => "critical",
        }
    }
}

/// How a single event contributes to the health burden.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventContribution {
    /// Event ID.
    pub id: String,

    /// Event title.
    pub title: String,

    /// Severity level.
    pub severity: Severity,

    /// Raw temporal frecency score `F_i` = Σ exp(-λ · `age_k`).
    pub temporal_frecency: f64,

    /// Penalty before cascade attribution: `P_raw` = `weight` × `frecency`.
    pub raw_penalty: f64,

    /// Fraction of this event's penalty attributed to a parent cascade.
    ///
    /// - 0.0  = independent event, full penalty applies
    /// - 0.85 = 85% attributed to parent (graph-confirmed cascade)
    /// - 0.65 = 65% attributed to parent (temporally-inferred cascade)
    pub cascade_attribution_fraction: f64,

    /// Penalty after cascade attenuation: `P_adj` = `P_raw` × (1 - `attribution`).
    pub adjusted_penalty: f64,

    /// This event's share of the total burden, as a percentage.
    pub burden_share_pct: f64,
}

/// How resource utilization contributes to the health burden.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourceContribution {
    /// CPU penalty (before weighting by `alpha_cpu`).
    pub cpu_penalty: f64,
    /// Memory penalty (before weighting by `alpha_memory`).
    pub memory_penalty: f64,
    /// Disk penalty (before weighting by `alpha_disk`).
    pub disk_penalty: f64,
    /// Load average penalty (before weighting by `alpha_load`).
    pub load_penalty: f64,
    /// Total weighted resource burden: Σ `alpha_j` × `R_j`.
    pub total: f64,
    /// Share of total burden as a percentage.
    pub burden_share_pct: f64,
    /// True if baseline learning is incomplete and static thresholds are in use.
    pub baseline_learning_mode: bool,
}
