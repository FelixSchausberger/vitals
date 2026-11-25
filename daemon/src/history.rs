//! Score history: rolling 7-day ring of (timestamp, score) pairs.
//!
//! Persisted to `~/.local/share/vitals/history.json` so history
//! survives daemon restarts. The daemon pushes a record after every
//! TWHS calculation and saves periodically.

use std::{
    collections::VecDeque,
    fs,
    path::{Path, PathBuf},
};

use anyhow::Result;
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

/// A single timestamped health score.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScoreRecord {
    /// Unix timestamp (seconds since epoch).
    pub timestamp: i64,
    /// TWHS score in (0, 100].
    pub score: f64,
}

/// Rolling 7-day history of TWHS scores.
///
/// Records are kept in chronological order. Old records beyond `max_age_secs`
/// are pruned automatically on each `push`.
pub struct ScoreHistory {
    records: VecDeque<ScoreRecord>,
    path: PathBuf,
    /// How many pushes since the last save (used to batch writes).
    dirty_count: u32,
}

/// Save to disk every N pushes (~5 minutes at 30s polling).
const SAVE_EVERY: u32 = 10;
/// Retain records for 7 days.
const MAX_AGE_SECS: i64 = 7 * 24 * 3600;

impl ScoreHistory {
    /// Load history from the default path, creating an empty history on error.
    #[must_use]
    pub fn load_or_new() -> Self {
        let path = default_path();
        match load_records(&path) {
            Ok(records) => Self {
                records,
                path,
                dirty_count: 0,
            },
            Err(_) => Self {
                records: VecDeque::new(),
                path,
                dirty_count: 0,
            },
        }
    }

    /// Push a new score and prune records older than 7 days.
    ///
    /// Automatically saves to disk every [`SAVE_EVERY`] pushes.
    pub fn push(&mut self, score: f64, now: OffsetDateTime) {
        self.records.push_back(ScoreRecord {
            timestamp: now.unix_timestamp(),
            score,
        });
        let cutoff = now.unix_timestamp() - MAX_AGE_SECS;
        while self.records.front().is_some_and(|r| r.timestamp < cutoff) {
            self.records.pop_front();
        }
        self.dirty_count += 1;
        if self.dirty_count >= SAVE_EVERY {
            if let Err(e) = self.save() {
                eprintln!("Warning: failed to save score history: {e}");
            }
            self.dirty_count = 0;
        }
    }

    /// Flush to disk immediately (call on daemon shutdown or first boot).
    ///
    /// # Errors
    ///
    /// Returns an error if the file cannot be written.
    pub fn save(&self) -> Result<()> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)?;
        }
        let json = serde_json::to_string(&self.records)?;
        fs::write(&self.path, json)?;
        Ok(())
    }

    /// All records, oldest first.
    #[must_use]
    pub fn all_records(&self) -> &VecDeque<ScoreRecord> {
        &self.records
    }

    /// Records within the last `hours` hours.
    #[must_use]
    pub fn records_last_hours(&self, hours: u64, now_ts: i64) -> Vec<&ScoreRecord> {
        let cutoff = now_ts - (i64::try_from(hours).unwrap_or(0) * 3600);
        self.records
            .iter()
            .filter(|r| r.timestamp >= cutoff)
            .collect()
    }

    /// Signed change from `period_secs` ago to the current score.
    ///
    /// Both ends are means over a window, not single samples. The daemon
    /// samples every 30 seconds, so one build spike at either end dominated a
    /// point comparison and reported a double-digit 24-hour drop for a host
    /// whose daily mean had barely moved. The window is capped at an hour so a
    /// 7-day comparison does not smear across unrelated days.
    ///
    /// Returns `None` if there is no record old enough to compare against.
    #[must_use]
    pub fn change_over_period(&self, period_secs: i64, now_score: f64, now_ts: i64) -> Option<f64> {
        let half_window = (period_secs / 12).clamp(300, 3600);
        let target_ts = now_ts - period_secs;

        let anchor = self.mean_between(target_ts - half_window, target_ts + half_window)?;
        let now = self
            .mean_between(now_ts - half_window, now_ts)
            .unwrap_or(now_score);

        Some(now - anchor)
    }

    /// Mean score of records inside `[start, end]`, or `None` when empty.
    fn mean_between(&self, start: i64, end: i64) -> Option<f64> {
        let mut sum = 0.0;
        let mut count = 0_u32;
        for record in &self.records {
            if record.timestamp >= start && record.timestamp <= end {
                sum += record.score;
                count += 1;
            }
        }
        (count > 0).then(|| sum / f64::from(count))
    }
}

fn default_path() -> PathBuf {
    dirs::data_local_dir()
        .unwrap_or_else(|| dirs::home_dir().unwrap_or_else(|| PathBuf::from(".")))
        .join("vitals")
        .join("history.json")
}

fn load_records(path: &Path) -> Result<VecDeque<ScoreRecord>> {
    let json = fs::read_to_string(path)?;
    Ok(serde_json::from_str(&json)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn history(samples: &[(i64, f64)]) -> ScoreHistory {
        ScoreHistory {
            records: samples
                .iter()
                .map(|&(timestamp, score)| ScoreRecord { timestamp, score })
                .collect(),
            path: PathBuf::from("/nonexistent/vitals/history.json"),
            dirty_count: 0,
        }
    }

    fn close(actual: f64, expected: f64) {
        assert!(
            (actual - expected).abs() < 1e-9,
            "expected {expected}, got {actual}"
        );
    }

    #[test]
    fn change_averages_both_ends_instead_of_point_samples() {
        let now = 1_800_000_000;
        let target = now - 3600;
        let history = history(&[
            (target - 60, 90.0),
            (target + 60, 92.0),
            (now - 120, 80.0),
            (now - 60, 82.0),
            (now, 60.0),
        ]);

        let change = history.change_over_period(3600, 60.0, now).expect("change");

        // Anchor mean 91, recent mean 74. The point comparison would have
        // read 60 - 92 = -32.
        close(change, -17.0);
    }

    #[test]
    fn change_is_none_without_an_anchor() {
        let now = 1_800_000_000;
        let history = history(&[(now - 60, 90.0), (now, 89.0)]);

        assert!(history.change_over_period(3600, 89.0, now).is_none());
    }

    #[test]
    fn missing_recent_samples_fall_back_to_the_current_score() {
        let now = 1_800_000_000;
        let target = now - 3600;
        let history = history(&[(target, 90.0)]);

        let change = history.change_over_period(3600, 77.0, now).expect("change");

        close(change, -13.0);
    }

    #[test]
    fn long_period_window_is_capped_at_one_hour() {
        let now = 1_800_000_000;
        let target = now - 7 * 86400;
        let history = history(&[(target - 5400, 50.0), (target + 60, 90.0), (now - 60, 80.0)]);

        let change = history
            .change_over_period(7 * 86400, 80.0, now)
            .expect("change");

        // The 50 two hours outside the anchor is excluded; the 90 is the
        // anchor mean and the 80 the recent mean.
        close(change, -10.0);
    }

    #[test]
    fn empty_history_has_no_change() {
        assert!(history(&[])
            .change_over_period(3600, 90.0, 1_800_000_000)
            .is_none());
    }
}
