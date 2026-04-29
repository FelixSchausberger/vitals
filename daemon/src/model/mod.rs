// Re-export core types for backward compatibility
pub use vitals_core::{Issue, IssueError, IssueTrend, Severity};

pub mod issue {
    pub use vitals_core::{Issue, IssueError, IssueTrend, Severity};
}

/// Time filter for log entries
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimeFilter {
    /// Show logs since last boot
    SinceBoot,
    /// Show logs from last 24 hours
    Last24h,
    /// Show logs from last week
    LastWeek,
    /// Show logs from last month
    LastMonth,
    /// Show all available logs
    All,
}

impl TimeFilter {
    /// Get the display name for this filter
    #[must_use]
    #[allow(dead_code)]
    pub const fn display_name(self) -> &'static str {
        match self {
            Self::SinceBoot => "since boot",
            Self::Last24h => "last 24h",
            Self::LastWeek => "last week",
            Self::LastMonth => "last month",
            Self::All => "all time",
        }
    }

    /// Cycle to the next filter
    #[must_use]
    pub const fn next(self) -> Self {
        match self {
            Self::SinceBoot => Self::Last24h,
            Self::Last24h => Self::LastWeek,
            Self::LastWeek => Self::LastMonth,
            Self::LastMonth => Self::All,
            Self::All => Self::SinceBoot,
        }
    }

    /// Get the time constraint for this filter
    #[must_use]
    #[allow(clippy::cast_possible_wrap)]
    pub fn get_since_time(self) -> Option<time::OffsetDateTime> {
        use time::OffsetDateTime;
        let now = OffsetDateTime::now_utc();

        match self {
            Self::SinceBoot => {
                #[cfg(test)]
                {
                    // Use deterministic boot time for tests (10 minutes ago)
                    Some(now - time::Duration::minutes(10))
                }
                #[cfg(not(test))]
                {
                    // Get actual system boot time
                    use sysinfo::System;
                    let uptime_secs = System::uptime();
                    Some(now - time::Duration::seconds(uptime_secs as i64))
                }
            }
            Self::Last24h => Some(now - time::Duration::hours(24)),
            Self::LastWeek => Some(now - time::Duration::weeks(1)),
            Self::LastMonth => Some(now - time::Duration::days(30)),
            Self::All => None, // No time constraint
        }
    }
}

impl Default for TimeFilter {
    fn default() -> Self {
        Self::SinceBoot
    }
}
