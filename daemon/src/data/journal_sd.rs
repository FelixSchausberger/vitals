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
/// Position is tracked with opaque sd-journal cursors rather than timestamps:
/// cursors survive rotation/vacuum and advance past records whose timestamps
/// cannot be parsed, so one malformed record can never wedge the reader into
/// re-reporting the same slice forever.
pub struct SystemdJournalReader {
    /// Opaque cursor of the last entry returned to the caller.
    /// `None` means no prior read — fall back to the caller's `since` window.
    last_cursor: Mutex<Option<String>>,
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
            last_cursor: Mutex::new(None),
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
            last_cursor: Mutex::new(None),
        })
    }

    /// Convert a `JournalRecord` to our `JournalEntry` format.
    ///
    /// Returns `None` for records without a usable `__REALTIME_TIMESTAMP` —
    /// the caller skips them. Stamping such records with the current time
    /// instead would resurrect stale entries as current issues on every tick
    /// (and stall the incremental cursor, re-reporting the same slice
    /// forever), so failing closed here is load-bearing.
    #[allow(clippy::cast_possible_wrap)]
    fn record_to_entry(record: &JournalRecord) -> Option<JournalEntry> {
        let realtime = record.get("__REALTIME_TIMESTAMP")?;
        let microseconds = realtime.parse::<u64>().ok()?;
        let system_time = UNIX_EPOCH + std::time::Duration::from_micros(microseconds);
        let timestamp = OffsetDateTime::from(system_time);

        let priority: u8 = record
            .get("PRIORITY")
            .and_then(|p| p.parse().ok())
            .unwrap_or(6);

        let message = record
            .get("MESSAGE")
            .map_or("(no message)".to_string(), |s| s.clone());

        let unit = record.get("_SYSTEMD_UNIT").cloned();
        let pid = record.get("_PID").and_then(|p| p.parse().ok());

        Some(JournalEntry {
            timestamp,
            priority,
            message,
            unit,
            pid,
        })
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
        //   1. If we have a previous read position, seek to it by opaque
        //      cursor and skip past it (incremental).
        //   2. Otherwise use the caller's `since` time (initial / reset read).
        //   3. If neither, seek to tail and walk backwards.
        let prior_cursor = self.last_cursor.lock().unwrap().clone();
        let mut skip_first = false;

        if let Some(cursor) = prior_cursor.as_deref() {
            // seek_cursor positions AT the saved entry; the first record
            // read is it, so skip exactly one entry below. (Verified by
            // comparing cursors rather than assuming seek/next semantics.)
            // A failed seek means the saved entry was vacuumed/rotated
            // away — fall through to the time-window read instead of
            // erroring the tick.
            skip_first = journal.seek_cursor(cursor).is_ok();
        }
        if !skip_first {
            if let Some(since_time) = since {
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
        }

        let mut entries = Vec::new();
        let mut skipped_no_timestamp: u64 = 0;
        let mut new_cursor: Option<String> = None;

        loop {
            if entries.len() >= limit {
                break;
            }

            match journal.next_entry() {
                Ok(Some(record)) => {
                    // Advance the cursor past every record successfully read —
                    // including ones skipped below — so a malformed record can
                    // never pin the reader to the same slice.
                    if let Ok(cursor) = journal.cursor() {
                        if skip_first {
                            skip_first = false;
                            // Skip the entry already returned last time; if it
                            // is already gone, this record is new — keep it.
                            if Some(cursor.as_str()) == prior_cursor.as_deref() {
                                new_cursor = Some(cursor);
                                continue;
                            }
                        }
                        new_cursor = Some(cursor);
                    }
                    match Self::record_to_entry(&record) {
                        Some(entry) => entries.push(entry),
                        None => skipped_no_timestamp += 1,
                    }
                }
                Ok(None) => break,
                Err(e) => {
                    eprintln!("Warning: Failed to read journal record: {e}");
                    break;
                }
            }
        }

        // Persist cursor for incremental reads. Stored even when zero entries
        // converted: the read position moved regardless.
        if let Some(cursor) = new_cursor {
            *self.last_cursor.lock().unwrap() = Some(cursor);
        }

        if skipped_no_timestamp > 0 {
            eprintln!(
                "vitals journal reader: skipped {skipped_no_timestamp} record(s) without usable timestamps"
            );
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
                    if let Some(entry) = Self::record_to_entry(&record) {
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

    fn test_record(pairs: &[(&str, &str)]) -> JournalRecord {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn test_record_without_timestamp_is_skipped_not_restamped() {
        // Regression test: records lacking __REALTIME_TIMESTAMP must NOT be
        // stamped with the current time — that resurrects stale entries as
        // current issues on every tick and stalls the incremental cursor.
        let record = test_record(&[
            (
                "MESSAGE",
                "comin-autopush.service: Failed with result 'exit-code'.",
            ),
            ("PRIORITY", "4"),
        ]);

        let result = SystemdJournalReader::record_to_entry(&record);
        assert!(
            result.is_none(),
            "timestamp-less record must be skipped, not restamped"
        );
    }

    #[test]
    fn test_record_with_unparsable_timestamp_is_skipped() {
        let record = test_record(&[
            ("__REALTIME_TIMESTAMP", "not-a-number"),
            ("MESSAGE", "boom"),
            ("PRIORITY", "3"),
        ]);

        let result = SystemdJournalReader::record_to_entry(&record);
        assert!(result.is_none());
    }

    #[test]
    #[allow(clippy::unreadable_literal)]
    fn test_record_with_valid_fields_converts() {
        let record = test_record(&[
            ("__REALTIME_TIMESTAMP", "1788522330387455"),
            ("MESSAGE", "hello"),
            ("PRIORITY", "4"),
            ("_SYSTEMD_UNIT", "foo.service"),
            ("_PID", "1234"),
        ]);

        let entry =
            SystemdJournalReader::record_to_entry(&record).expect("valid record must convert");
        assert_eq!(entry.message, "hello");
        assert_eq!(entry.priority, 4);
        assert_eq!(entry.unit.as_deref(), Some("foo.service"));
        assert_eq!(entry.pid, Some(1234));
        assert_eq!(entry.timestamp.unix_timestamp(), 1788522330);
    }

    #[test]
    #[allow(clippy::unreadable_literal)]
    fn test_record_missing_optional_fields_uses_defaults() {
        let record = test_record(&[("__REALTIME_TIMESTAMP", "1788522330387455")]);

        let entry = SystemdJournalReader::record_to_entry(&record)
            .expect("record with only a timestamp must convert");
        assert_eq!(entry.message, "(no message)");
        assert_eq!(entry.priority, 6);
        assert_eq!(entry.unit, None);
        assert_eq!(entry.pid, None);
    }
}
