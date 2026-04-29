//! Threshold-based notifier for health score events.
//!
//! Emits structured log entries (via `tracing`) when the health score
//! crosses the configured threshold. Uses a cooldown to avoid spamming.

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

use crate::health::HealthBreakdown;

/// Notifier configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NotifierConfig {
    /// Alert when score drops below this threshold (0-100)
    pub alert_below: f64,
    /// Minimum seconds between repeated alerts for the same condition
    pub cooldown_secs: u64,
}

impl Default for NotifierConfig {
    fn default() -> Self {
        Self {
            alert_below: 75.0,
            cooldown_secs: 1800,
        }
    }
}

/// Whether we are currently in a below-threshold alert state
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AlertState {
    Normal,
    Alerting,
}

pub struct Notifier {
    config: NotifierConfig,
    state: AlertState,
    last_alert_ts: Option<OffsetDateTime>,
    last_recovery_ts: Option<OffsetDateTime>,
}

impl Notifier {
    #[must_use]
    pub fn new(config: NotifierConfig) -> Self {
        Self {
            config,
            state: AlertState::Normal,
            last_alert_ts: None,
            last_recovery_ts: None,
        }
    }

    /// Called after each health score calculation.
    /// Returns true if a notification was emitted.
    pub fn notify(
        &mut self,
        score: f64,
        delta_1h: Option<f64>,
        breakdown: &HealthBreakdown,
    ) -> bool {
        let now = OffsetDateTime::now_utc();
        let threshold = self.config.alert_below;
        let cooldown_secs = i64::try_from(self.config.cooldown_secs).unwrap_or(0);

        match self.state {
            AlertState::Normal => {
                if score < threshold {
                    if let Some(ts) = self.last_alert_ts {
                        if now.unix_timestamp() - ts.unix_timestamp() < cooldown_secs {
                            return false;
                        }
                    }
                    self.state = AlertState::Alerting;
                    self.last_alert_ts = Some(now);
                    self.emit_alert(score, delta_1h, breakdown);
                    return true;
                }
            }
            AlertState::Alerting => {
                if score >= threshold {
                    if let Some(ts) = self.last_recovery_ts {
                        if now.unix_timestamp() - ts.unix_timestamp() < cooldown_secs / 2 {
                            return false;
                        }
                    }
                    self.state = AlertState::Normal;
                    self.last_recovery_ts = Some(now);
                    self.emit_recovery(score, delta_1h);
                    return true;
                }
            }
        }
        false
    }

    fn emit_alert(&self, score: f64, delta_1h: Option<f64>, breakdown: &HealthBreakdown) {
        let top_issue = breakdown
            .issue_impacts
            .first()
            .map_or("no issues".to_string(), |i| {
                format!(
                    "{} ({}x{})",
                    i.title,
                    format!("{:?}", i.severity).to_lowercase(),
                    i.count
                )
            });

        eprintln!(
            "ALERT: score={:.1} threshold={:.1} delta_1h={:.1} errors={} warnings={} top_issue={}",
            score,
            self.config.alert_below,
            delta_1h.unwrap_or(0.0),
            breakdown.error_count,
            breakdown.warning_count,
            top_issue
        );
    }

    fn emit_recovery(&self, score: f64, delta_1h: Option<f64>) {
        eprintln!(
            "RECOVERY: score={:.1} threshold={:.1} delta_1h={:.1}",
            score,
            self.config.alert_below,
            delta_1h.unwrap_or(0.0)
        );
    }
}
