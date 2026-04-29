//! Log parser module for categorizing journal entries by priority level.
//!
//! Provides utilities to parse and categorize journal entries as Error, Warning, or Other
//! based on their syslog priority levels.

use crate::{data::traits::JournalEntry, model::Severity};

/// Log entry category based on priority level
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogCategory {
    /// Errors (priorities 0-3: Emergency, Alert, Critical, Error)
    Error,
    /// Warnings (priority 4: Warning)
    Warning,
    /// Other entries (priorities 5-7: Notice, Info, Debug)
    Other,
}

impl LogCategory {
    /// Parse priority level to log category
    #[must_use]
    pub const fn from_priority(priority: u8) -> Self {
        match priority {
            0..=3 => Self::Error, // Emergency, Alert, Critical, Error
            4 => Self::Warning,   // Warning
            _ => Self::Other,     // Notice, Info, Debug
        }
    }

    /// Convert to severity for issue creation
    #[allow(dead_code)]
    #[must_use]
    pub const fn to_severity(self) -> Option<Severity> {
        match self {
            Self::Error => Some(Severity::Error),
            Self::Warning => Some(Severity::Warning),
            Self::Other => None, // We don't create issues for info/debug
        }
    }

    /// Get display name
    #[allow(dead_code)]
    #[must_use]
    pub const fn display_name(self) -> &'static str {
        match self {
            Self::Error => "Error",
            Self::Warning => "Warning",
            Self::Other => "Info",
        }
    }
}

/// Parse and categorize a journal entry
#[must_use]
pub const fn categorize_entry(entry: &JournalEntry) -> LogCategory {
    LogCategory::from_priority(entry.priority)
}

/// Count entries by category from a collection
#[must_use]
pub fn count_by_category(entries: &[JournalEntry]) -> (usize, usize, usize) {
    let mut error_count = 0;
    let mut warning_count = 0;
    let mut other_count = 0;

    for entry in entries {
        match categorize_entry(entry) {
            LogCategory::Error => error_count += 1,
            LogCategory::Warning => warning_count += 1,
            LogCategory::Other => other_count += 1,
        }
    }

    (error_count, warning_count, other_count)
}

/// Filter entries by category
#[allow(dead_code)]
#[must_use]
pub fn filter_by_category(entries: &[JournalEntry], category: LogCategory) -> Vec<&JournalEntry> {
    entries
        .iter()
        .filter(|entry| categorize_entry(entry) == category)
        .collect()
}

#[cfg(test)]
mod tests {
    use time::macros::datetime;

    use super::*;

    #[test]
    fn test_priority_categorization() {
        assert_eq!(LogCategory::from_priority(0), LogCategory::Error); // Emergency
        assert_eq!(LogCategory::from_priority(1), LogCategory::Error); // Alert
        assert_eq!(LogCategory::from_priority(2), LogCategory::Error); // Critical
        assert_eq!(LogCategory::from_priority(3), LogCategory::Error); // Error
        assert_eq!(LogCategory::from_priority(4), LogCategory::Warning); // Warning
        assert_eq!(LogCategory::from_priority(5), LogCategory::Other); // Notice
        assert_eq!(LogCategory::from_priority(6), LogCategory::Other); // Info
        assert_eq!(LogCategory::from_priority(7), LogCategory::Other); // Debug
    }

    #[test]
    fn test_categorize_entry() {
        let error_entry = JournalEntry {
            timestamp: datetime!(2024-09-15 10:00:00 UTC),
            priority: 3, // Error
            message: "Critical error".to_string(),
            unit: Some("test.service".to_string()),
            pid: Some(123),
        };

        let warning_entry = JournalEntry {
            timestamp: datetime!(2024-09-15 10:01:00 UTC),
            priority: 4, // Warning
            message: "Warning message".to_string(),
            unit: Some("test.service".to_string()),
            pid: Some(124),
        };

        let info_entry = JournalEntry {
            timestamp: datetime!(2024-09-15 10:02:00 UTC),
            priority: 6, // Info
            message: "Info message".to_string(),
            unit: Some("test.service".to_string()),
            pid: Some(125),
        };

        assert_eq!(categorize_entry(&error_entry), LogCategory::Error);
        assert_eq!(categorize_entry(&warning_entry), LogCategory::Warning);
        assert_eq!(categorize_entry(&info_entry), LogCategory::Other);
    }

    #[test]
    fn test_count_by_category() {
        let entries = vec![
            JournalEntry {
                timestamp: datetime!(2024-09-15 10:00:00 UTC),
                priority: 3, // Error
                message: "Error 1".to_string(),
                unit: Some("test.service".to_string()),
                pid: Some(123),
            },
            JournalEntry {
                timestamp: datetime!(2024-09-15 10:01:00 UTC),
                priority: 3, // Error
                message: "Error 2".to_string(),
                unit: Some("test.service".to_string()),
                pid: Some(124),
            },
            JournalEntry {
                timestamp: datetime!(2024-09-15 10:02:00 UTC),
                priority: 4, // Warning
                message: "Warning 1".to_string(),
                unit: Some("test.service".to_string()),
                pid: Some(125),
            },
            JournalEntry {
                timestamp: datetime!(2024-09-15 10:03:00 UTC),
                priority: 6, // Info
                message: "Info 1".to_string(),
                unit: Some("test.service".to_string()),
                pid: Some(126),
            },
        ];

        let (error_count, warning_count, other_count) = count_by_category(&entries);

        assert_eq!(error_count, 2);
        assert_eq!(warning_count, 1);
        assert_eq!(other_count, 1);
    }
}
