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

    /// Signed change from `period_secs` ago to `now_score`.
    ///
    /// Returns `None` if there is no record old enough to compare against.
    #[must_use]
    pub fn change_over_period(&self, period_secs: i64, now_score: f64, now_ts: i64) -> Option<f64> {
        let target_ts = now_ts - period_secs;
        // Accept any record within 10% of the target period as a valid anchor.
        let tolerance = (period_secs / 10).max(300); // at least 5 minutes
        self.records
            .iter()
            .min_by_key(|r| (r.timestamp - target_ts).unsigned_abs())
            .filter(|r| (r.timestamp - target_ts).abs() <= tolerance)
            .map(|r| now_score - r.score)
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
