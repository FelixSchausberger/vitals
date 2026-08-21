//! Log entry collection, aggregation, and querying for the `/logs` endpoint.
//!
//! Raw [`JournalEntry`] values are converted to API [`LogEntry`] values with
//! duplicate messages collapsed into a single aggregated entry carrying an
//! occurrence count. [`apply_query`] implements the server-side filtering and
//! pagination described by [`LogsQuery`].

use std::collections::HashMap;

use vitals_core::{
    api::{LogEntry, LogsQuery, TimeWindow},
    Severity,
};

use crate::data::{
    parser::{categorize_entry, LogCategory},
    traits::JournalEntry,
};

/// Grouping key for duplicate log entries.
type EntryKey = (Option<String>, Option<u32>, u8, String);

/// Accumulated state for one group of duplicate entries.
struct Group {
    severity: Severity,
    message: String,
    unit: Option<String>,
    pid: Option<u32>,
    count: usize,
    first_seen: time::OffsetDateTime,
    last_seen: time::OffsetDateTime,
}

/// Convert raw journal entries into API log entries.
///
/// Entries sharing the same `(unit, pid, priority, message)` are collapsed
/// into one entry whose `count`, `first_seen`, and `last_seen` describe the
/// whole group. Input order is preserved by first occurrence.
#[must_use]
pub fn collect_log_entries(entries: &[JournalEntry], window: &TimeWindow) -> Vec<LogEntry> {
    let mut groups: Vec<Group> = Vec::new();
    let mut index: HashMap<EntryKey, usize> = HashMap::new();

    for entry in entries {
        let key = (
            entry.unit.clone(),
            entry.pid,
            entry.priority,
            entry.message.clone(),
        );
        if let Some(&i) = index.get(&key) {
            let group = &mut groups[i];
            group.count += 1;
            if entry.timestamp < group.first_seen {
                group.first_seen = entry.timestamp;
            }
            if entry.timestamp > group.last_seen {
                group.last_seen = entry.timestamp;
            }
        } else {
            index.insert(key, groups.len());
            groups.push(Group {
                severity: severity_of(entry),
                message: entry.message.clone(),
                unit: entry.unit.clone(),
                pid: entry.pid,
                count: 1,
                first_seen: entry.timestamp,
                last_seen: entry.timestamp,
            });
        }
    }

    groups
        .into_iter()
        .map(|g| LogEntry {
            timestamp: g.last_seen,
            message: g.message,
            severity: g.severity,
            unit: g.unit,
            pid: g.pid,
            count: g.count,
            first_seen: Some(g.first_seen),
            last_seen: Some(g.last_seen),
        })
        .filter(|entry| entry.timestamp >= window.start && entry.timestamp <= window.end)
        .collect()
}

/// Apply severity/unit/time filters, then `offset`/`limit` pagination.
///
/// Returns the total number of matching entries (before pagination) and the
/// requested page.
#[must_use]
pub fn apply_query(entries: Vec<LogEntry>, query: &LogsQuery) -> (usize, Vec<LogEntry>) {
    let filtered: Vec<LogEntry> = entries
        .into_iter()
        .filter(|entry| {
            query.severity.is_none_or(|s| entry.severity == s.into())
                && query
                    .unit
                    .as_deref()
                    .is_none_or(|u| entry.unit.as_deref() == Some(u))
                && query.since.is_none_or(|t| entry.timestamp >= t)
                && query.until.is_none_or(|t| entry.timestamp <= t)
        })
        .collect();

    let total = filtered.len();
    let start = query.offset.min(total);
    let end = query
        .limit
        .map_or(total, |l| start.saturating_add(l).min(total));

    (total, filtered[start..end].to_vec())
}

fn severity_of(entry: &JournalEntry) -> Severity {
    match categorize_entry(entry) {
        LogCategory::Error => Severity::Error,
        LogCategory::Warning => Severity::Warning,
        LogCategory::Other => Severity::Info,
    }
}

#[cfg(test)]
mod tests {
    use time::macros::datetime;

    use super::*;

    fn window(start: time::OffsetDateTime, end: time::OffsetDateTime) -> TimeWindow {
        TimeWindow {
            start,
            end,
            description: "test".to_string(),
        }
    }

    fn entry(ts: (time::Month, u8), priority: u8, message: &str, unit: &str) -> JournalEntry {
        JournalEntry {
            timestamp: datetime!(2024-09-15 10:00:00 UTC)
                .replace_month(ts.0)
                .expect("valid month")
                .replace_hour(ts.1)
                .expect("valid hour"),
            priority,
            message: message.to_string(),
            unit: Some(unit.to_string()),
            pid: Some(42),
        }
    }

    fn query(
        severity: Option<vitals_core::api::SeverityFilter>,
        unit: Option<&str>,
        limit: Option<usize>,
        offset: usize,
    ) -> LogsQuery {
        LogsQuery {
            severity,
            unit: unit.map(ToOwned::to_owned),
            since: None,
            until: None,
            limit,
            offset,
        }
    }

    #[test]
    fn duplicates_are_aggregated_with_count_and_bounds() {
        let w = window(
            datetime!(2024-09-15 00:00:00 UTC),
            datetime!(2024-09-16 00:00:00 UTC),
        );
        let entries = vec![
            entry((time::Month::September, 3), 3, "disk failure", "sd.service"),
            entry((time::Month::September, 5), 3, "disk failure", "sd.service"),
            entry((time::Month::September, 4), 4, "high memory", "app.service"),
        ];

        let logs = collect_log_entries(&entries, &w);

        assert_eq!(logs.len(), 2);
        let disk = logs
            .iter()
            .find(|l| l.message == "disk failure")
            .expect("disk entry");
        assert_eq!(disk.count, 2);
        assert_eq!(disk.first_seen, Some(datetime!(2024-09-15 03:00:00 UTC)));
        assert_eq!(disk.last_seen, Some(datetime!(2024-09-15 05:00:00 UTC)));
        assert_eq!(disk.severity, Severity::Error);

        let mem = logs
            .iter()
            .find(|l| l.message == "high memory")
            .expect("mem entry");
        assert_eq!(mem.count, 1);
        assert_eq!(mem.severity, Severity::Warning);
    }

    #[test]
    fn entries_outside_window_are_dropped() {
        let w = window(
            datetime!(2024-09-15 04:00:00 UTC),
            datetime!(2024-09-16 00:00:00 UTC),
        );
        let entries = vec![
            entry((time::Month::September, 3), 3, "early", "a.service"),
            entry((time::Month::September, 5), 3, "late", "a.service"),
        ];

        let logs = collect_log_entries(&entries, &w);
        assert_eq!(logs.len(), 1);
        assert_eq!(logs[0].message, "late");
    }

    #[test]
    fn filters_narrow_results() {
        let w = window(
            datetime!(2024-09-15 00:00:00 UTC),
            datetime!(2024-09-16 00:00:00 UTC),
        );
        let entries = vec![
            entry((time::Month::September, 1), 3, "err", "a.service"),
            entry((time::Month::September, 2), 4, "warn", "b.service"),
            entry((time::Month::September, 3), 6, "info", "a.service"),
        ];
        let logs = collect_log_entries(&entries, &w);

        let (total, page) = apply_query(
            logs,
            &query(Some(vitals_core::api::SeverityFilter::Error), None, None, 0),
        );
        assert_eq!(total, 1);
        assert_eq!(page[0].message, "err");

        let logs = collect_log_entries(&entries, &w);
        let (total, _) = apply_query(logs, &query(None, Some("a.service"), None, 0));
        assert_eq!(total, 2);
    }

    #[test]
    fn pagination_slices_after_filtering() {
        let w = window(
            datetime!(2024-09-15 00:00:00 UTC),
            datetime!(2024-09-16 00:00:00 UTC),
        );
        // Five distinct messages so nothing collapses.
        let entries: Vec<JournalEntry> = (1u8..=5)
            .map(|h| {
                entry(
                    (time::Month::September, h),
                    3,
                    &format!("boom{h}"),
                    "x.service",
                )
            })
            .collect();

        let logs = collect_log_entries(&entries, &w);
        let (total, page) = apply_query(logs, &query(None, None, Some(2), 1));
        assert_eq!(total, 5);
        assert_eq!(page.len(), 2);
        assert_eq!(page[0].message, "boom2");
        assert_eq!(page[1].message, "boom3");
    }

    #[test]
    fn offset_beyond_total_yields_empty_page() {
        let w = window(
            datetime!(2024-09-15 00:00:00 UTC),
            datetime!(2024-09-16 00:00:00 UTC),
        );
        let entries = vec![entry((time::Month::September, 1), 3, "only", "x.service")];
        let logs = collect_log_entries(&entries, &w);

        let (total, page) = apply_query(logs, &query(None, None, None, 10));
        assert_eq!(total, 1);
        assert!(page.is_empty());
    }
}
