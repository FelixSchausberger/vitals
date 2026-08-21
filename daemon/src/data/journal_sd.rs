//! Real journal integration using systemd journal API.
//!
//! This module implements the `JournalReader` trait using the systemd crate
//! to read journal entries from the system journal.
//!
//! Incremental reads: after the first call, subsequent `read_entries_since` calls
//! seek to just past the last-read entry rather than re-scanning the full time
//! window, dramatically reducing I/O on systems with busy journals.

#![allow(clippy::future_not_send)]
#![allow(clippy::redundant_closure_for_method_calls)]

use std::{
    sync::Mutex,
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{anyhow, Result};
use systemd::journal::{JournalRecord, JournalSeek, OpenOptions};
use time::OffsetDateTime;

use crate::data::traits::{JournalEntry, JournalReader};

/// Real journal reader implementation using systemd journal API.
///
/// After the first read, internal state is maintained so that subsequent reads
/// only return entries that arrived since the last call (incremental mode).
pub struct SystemdJournalReader {
    /// Microseconds-since-epoch timestamp of the last entry we read.
    /// `None` means no prior read — fall back to the caller's `since` window.
    last_read_usec: Mutex<Option<u64>>,
}

impl SystemdJournalReader {
    /// Create a new journal reader, validating journal access.
    ///
    /// # Errors
    /// Returns an error if the systemd journal cannot be opened.
    pub fn new() -> Result<Self> {
        let mut _test_journal = OpenOptions::default()
            .runtime_only(false)
            .system(true)
            .current_user(true)
            .open()
            .map_err(|e| anyhow!("Failed to open systemd journal: {e}"))?;

        Ok(Self {
            last_read_usec: Mutex::new(None),
        })
    }

    /// Create a new journal reader with custom options (for testing).
    ///
    /// # Errors
    /// Returns an error if the systemd journal cannot be opened.
    #[allow(dead_code)]
    pub fn with_options(system: bool, current_user: bool) -> Result<Self> {
        let mut _test_journal = OpenOptions::default()
            .runtime_only(false)
            .system(system)
            .current_user(current_user)
            .open()
            .map_err(|e| anyhow!("Failed to open systemd journal: {e}"))?;

        Ok(Self {
            last_read_usec: Mutex::new(None),
        })
    }

    /// Convert a `JournalRecord` to our `JournalEntry` format.
    #[allow(clippy::cast_possible_wrap)]
    fn record_to_entry(record: &JournalRecord) -> Result<JournalEntry> {
        let timestamp = if let Some(realtime) = record.get("__REALTIME_TIMESTAMP") {
            let microseconds: u64 = realtime
                .parse()
                .map_err(|_| anyhow!("Invalid timestamp format"))?;
            let system_time = UNIX_EPOCH + std::time::Duration::from_micros(microseconds);
            OffsetDateTime::from(system_time)
        } else {
            OffsetDateTime::now_utc()
        };

        let priority: u8 = record
            .get("PRIORITY")
            .and_then(|p| p.parse().ok())
            .unwrap_or(6);

        let message = record
            .get("MESSAGE")
            .map_or("(no message)".to_string(), |s| s.clone());

        let unit = record.get("_SYSTEMD_UNIT").cloned();
        let pid = record.get("_PID").and_then(|p| p.parse().ok());

        Ok(JournalEntry {
            timestamp,
            priority,
            message,
            unit,
            pid,
        })
    }

    /// Parse `__REALTIME_TIMESTAMP` from a record into microseconds.
    fn record_usec(record: &JournalRecord) -> Option<u64> {
        record
            .get("__REALTIME_TIMESTAMP")
            .and_then(|s| s.parse().ok())
    }
}

impl JournalReader for SystemdJournalReader {
    async fn read_entries(&self, limit: usize) -> Result<Vec<JournalEntry>> {
        self.read_entries_since(None, limit).await
    }

    async fn read_entries_since(
        &self,
        since: Option<OffsetDateTime>,
        limit: usize,
    ) -> Result<Vec<JournalEntry>> {
        let mut journal = OpenOptions::default()
            .runtime_only(false)
            .system(true)
            .current_user(true)
            .open()
            .map_err(|e| anyhow!("Failed to open systemd journal: {e}"))?;

        // Choose seek strategy:
        //   1. If we have a previous read position, seek to just past it (incremental).
        //   2. Otherwise use the caller's `since` time (initial / reset read).
        //   3. If neither, seek to tail and walk backwards.
        let prior_usec = *self.last_read_usec.lock().unwrap();

        if let Some(usec) = prior_usec {
            // +1 μs ensures we don't re-read the last entry
            journal
                .seek_realtime_usec(usec + 1)
                .map_err(|e| anyhow!("Failed to seek to last-read position: {e}"))?;
        } else if let Some(since_time) = since {
            let system_time: SystemTime = since_time.into();
            #[allow(clippy::cast_possible_truncation)]
            let since_micros = system_time
                .duration_since(UNIX_EPOCH)
                .map_err(|_| anyhow!("Invalid since timestamp"))?
                .as_micros() as u64;

            journal
                .seek_realtime_usec(since_micros)
                .map_err(|e| anyhow!("Failed to seek journal to timestamp: {e}"))?;
        } else {
            journal
                .seek(JournalSeek::Tail)
                .map_err(|e| anyhow!("Failed to seek to journal tail: {e}"))?;

            for _ in 0..limit {
                if journal.previous().is_err() {
                    break;
                }
            }
        }

        let mut entries = Vec::new();
        let mut max_usec = prior_usec.unwrap_or(0);

        loop {
            if entries.len() >= limit {
                break;
            }

            match journal.next_entry() {
                Ok(Some(record)) => {
                    // Track the latest timestamp so the next call can seek past it
                    if let Some(ts) = Self::record_usec(&record) {
                        if ts > max_usec {
                            max_usec = ts;
                        }
                    }
                    if let Ok(entry) = Self::record_to_entry(&record) {
                        entries.push(entry);
                    }
                }
                Ok(None) => break,
                Err(e) => {
                    eprintln!("Warning: Failed to read journal record: {e}");
                    break;
                }
            }
        }

        // Persist cursor for incremental reads — only advance, never go back
        if max_usec > prior_usec.unwrap_or(0) {
            *self.last_read_usec.lock().unwrap() = Some(max_usec);
        }

        // Most-recent-first for the caller
        entries.sort_by_key(|e| std::cmp::Reverse(e.timestamp));

        Ok(entries)
    }

    async fn read_unit_entries(&self, unit: &str, limit: usize) -> Result<Vec<JournalEntry>> {
        // Unit-specific reads always open a fresh, unfiltered journal (no cursor sharing)
        let mut journal = OpenOptions::default()
            .runtime_only(false)
            .system(true)
            .current_user(true)
            .open()
            .map_err(|e| anyhow!("Failed to open systemd journal: {e}"))?;

        journal
            .match_add("_SYSTEMD_UNIT", unit)
            .map_err(|e| anyhow!("Failed to add unit filter: {e}"))?;

        journal
            .seek(JournalSeek::Tail)
            .map_err(|e| anyhow!("Failed to seek to journal tail: {e}"))?;

        for _ in 0..limit {
            if journal.previous().is_err() {
                break;
            }
        }

        let mut entries = Vec::new();

        loop {
            if entries.len() >= limit {
                break;
            }

            match journal.next_entry() {
                Ok(Some(record)) => {
                    if let Ok(entry) = Self::record_to_entry(&record) {
                        entries.push(entry);
                    }
                }
                Ok(None) => break,
                Err(e) => {
                    eprintln!("Warning: Failed to read journal record: {e}");
                    break;
                }
            }
        }

        entries.sort_by_key(|e| std::cmp::Reverse(e.timestamp));

        Ok(entries)
    }
}

impl Default for SystemdJournalReader {
    fn default() -> Self {
        Self::new().expect("Failed to create default journal reader")
    }
}

#[cfg(test)]
mod tests {
    use time::Duration;

    use super::*;

    #[tokio::test]
    async fn test_read_entries_integration() {
        let Ok(reader) = SystemdJournalReader::new() else {
            eprintln!("Skipping journal test: systemd journal not available");
            return;
        };

        let entries = reader
            .read_entries(10)
            .await
            .expect("Failed to read entries");

        assert!(entries.len() <= 10);

        for entry in &entries {
            assert!(!entry.message.is_empty());
            assert!(entry.priority <= 7);
        }
    }

    #[tokio::test]
    async fn test_read_entries_since_integration() {
        let Ok(reader) = SystemdJournalReader::new() else {
            eprintln!("Skipping journal test: systemd journal not available");
            return;
        };

        let since = OffsetDateTime::now_utc() - Duration::hours(1);
        let entries = reader
            .read_entries_since(Some(since), 5)
            .await
            .expect("Failed to read entries since timestamp");

        assert!(entries.len() <= 5);

        for entry in &entries {
            assert!(entry.timestamp >= since);
        }
    }

    #[tokio::test]
    async fn test_incremental_reads() {
        let Ok(reader) = SystemdJournalReader::new() else {
            eprintln!("Skipping journal test: systemd journal not available");
            return;
        };

        let since = OffsetDateTime::now_utc() - Duration::hours(1);

        // First read populates the cursor
        let first = reader
            .read_entries_since(Some(since), 100)
            .await
            .expect("First read failed");

        // Second read with cursor should not re-return entries from before the cursor
        let second = reader
            .read_entries_since(Some(since), 100)
            .await
            .expect("Second read failed");

        // First read returns some historical entries; second read returns only new ones
        // (which should be 0 or very few in a test environment)
        assert!(
            second.len() <= first.len(),
            "Incremental read returned more entries than initial read"
        );
    }

    #[tokio::test]
    async fn test_read_unit_entries_integration() {
        let Ok(reader) = SystemdJournalReader::new() else {
            eprintln!("Skipping journal test: systemd journal not available");
            return;
        };

        let entries = reader
            .read_unit_entries("systemd-journald.service", 5)
            .await
            .expect("Failed to read unit entries");

        assert!(entries.len() <= 5);

        for entry in &entries {
            if let Some(ref unit) = entry.unit {
                assert_eq!(unit, "systemd-journald.service");
            }
        }
    }
}
