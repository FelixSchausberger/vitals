//! Issue aggregation logic.
//!
//! Converts raw journal entries and systemd unit states into structured
//! issues for the TUI to display.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use thiserror::Error;
use time::OffsetDateTime;

use crate::{
    data::traits::{JournalEntry, SystemdUnit},
    model::{Issue, Severity},
};

const BOOT_NOISE_PATTERNS: &[&str] = &[
    "MDS CPU bug present",
    "MMIO Stale Data CPU bug",
    "VMSCAPE:",
    "hpet_acpi_add:",
    "ENERGY_PERF_BIAS:",
    "spl: loading out-of-tree module taints kernel",
    "device-mapper: core: CONFIG_IMA_DISABLE_HTABLE",
    "zfs: module license",
    "Disabling lock debugging due to kernel taint",
    "NOTICE: Automounting of tracing to debugfs",
    "x86/cpu: SGX disabled",
    "spi-nor ",
];

/// Markers identifying a clean shutdown/reboot sequence. Suppression of
/// shutdown fallout is gated on one of these appearing in the same journal
/// window, so identical messages from a real crash are still scored.
const SHUTDOWN_MARKER_PATTERNS: &[&str] = &[
    "Shutting down",
    "Starting reboot",
    "systemd-shutdown",
    "target shutdown",
    "target Shutdown",
    "System Shutdown",
    "target reboot",
    "target Reboot",
    "reboot:",
];

/// Journal messages that are expected fallout of a clean shutdown/reboot
/// (graceful SIGTERM teardown, resolver losing its local upstream, Redis
/// exit notice, units killed by the shutdown transaction). Only suppressed
/// near a shutdown marker; elsewhere they score normally.
const SHUTDOWN_TRANSIENT_PATTERNS: &[&str] = &[
    "Got sig[15] terminate",
    "SIGTERM",
    "killed by signal 15",
    "Using degraded feature set UDP instead of TCP",
    "ready to exit, bye bye",
    "Failed with result 'exit-code'",
];

fn default_shutdown_grace_secs() -> i64 {
    120
}

fn default_shutdown_markers() -> Vec<String> {
    SHUTDOWN_MARKER_PATTERNS
        .iter()
        .map(ToString::to_string)
        .collect()
}

fn default_shutdown_transient_patterns() -> Vec<String> {
    SHUTDOWN_TRANSIENT_PATTERNS
        .iter()
        .map(ToString::to_string)
        .collect()
}

/// Errors that can occur during issue aggregation.
#[derive(Error, Debug)]
#[allow(dead_code)]
pub enum AggregationError {
    /// No entries provided for aggregation
    #[error("No journal entries or systemd units provided")]
    NoData,

    /// Invalid entry data
    #[error("Invalid entry data: {0}")]
    InvalidData(String),
}

/// Configuration for issue aggregation behavior.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AggregationConfig {
    /// Maximum number of hint messages to include per issue
    pub max_hints: usize,
    /// Whether to include process IDs in issues
    pub include_pids: bool,
    /// Time window for grouping related entries (in minutes)
    pub grouping_window_minutes: i64,
    /// Whether to enable sophisticated issue correlation
    pub enable_correlation: bool,
    /// Correlation similarity threshold (0.0-1.0)
    pub correlation_threshold: f64,
    /// Maximum number of issues to correlate together
    pub max_correlation_group_size: usize,
    /// Grace window around detected shutdown/reboot markers in seconds.
    /// Shutdown-transient entries inside this window are dropped before
    /// scoring. Zero disables the suppression.
    #[serde(default = "default_shutdown_grace_secs")]
    pub shutdown_grace_secs: i64,
    /// Message patterns identifying a clean shutdown/reboot sequence.
    #[serde(default = "default_shutdown_markers")]
    pub shutdown_markers: Vec<String>,
    /// Message patterns treated as shutdown fallout. Only suppressed near
    /// a shutdown marker; identical messages elsewhere score normally.
    #[serde(default = "default_shutdown_transient_patterns")]
    pub shutdown_transient_patterns: Vec<String>,
}

impl Default for AggregationConfig {
    fn default() -> Self {
        Self {
            max_hints: 3,
            include_pids: true,
            grouping_window_minutes: 5,
            enable_correlation: true,
            correlation_threshold: 0.7,
            max_correlation_group_size: 5,
            shutdown_grace_secs: default_shutdown_grace_secs(),
            shutdown_markers: default_shutdown_markers(),
            shutdown_transient_patterns: default_shutdown_transient_patterns(),
        }
    }
}

/// Aggregate journal entries and systemd units into issues.
///
/// # Arguments
/// * `journal_entries` - Journal entries to process
/// * `systemd_units` - Systemd units to check
///
/// # Returns
/// Vector of issues sorted by severity, time, and count
#[must_use]
#[allow(dead_code)] // Used by tests and some UI code
pub fn aggregate_issues(
    journal_entries: &[JournalEntry],
    systemd_units: &[SystemdUnit],
) -> Vec<Issue> {
    aggregate_issues_with_config(
        journal_entries,
        systemd_units,
        &AggregationConfig::default(),
    )
}

/// Aggregate issues with custom configuration.
#[must_use]
pub fn aggregate_issues_with_config(
    journal_entries: &[JournalEntry],
    systemd_units: &[SystemdUnit],
    config: &AggregationConfig,
) -> Vec<Issue> {
    let mut issues = Vec::new();
    let now = OffsetDateTime::now_utc();

    // Create a PID to unit mapping for enhanced process correlation
    let pid_to_unit_map = create_pid_to_unit_mapping(systemd_units);

    // Rule 1: Failed systemd units -> Error
    for unit in systemd_units {
        if unit.active_state == "failed" {
            let issue = create_systemd_issue(unit, now, config);
            issues.push(issue);
        }
    }

    // Drop known one-time boot noise that is not actionable and otherwise
    // dominates score impact after each reboot.
    // Drop clean-shutdown fallout (SIGTERM teardown, resolver losing its
    // local upstream, Redis exit notices) when a shutdown/reboot marker is
    // present in the same window. Entries stay visible via /logs; they just
    // don't become scored issues.
    let marker_times = shutdown_marker_times(journal_entries, config);
    let filtered_entries: Vec<JournalEntry> = journal_entries
        .iter()
        .filter(|entry| {
            !is_boot_noise(entry) && !is_shutdown_transient(entry, &marker_times, config)
        })
        .cloned()
        .collect();

    // Enhanced journal entry processing with better unit mapping
    let grouped_entries = group_journal_entries_enhanced(&filtered_entries, &pid_to_unit_map);

    // Process grouped entries using iteration patterns
    for ((priority, unit), entries) in grouped_entries {
        // Rule 2: Journal entries with priority ≤ 3 (Emergency, Alert, Critical, Error) -> Error
        // Rule 3: Journal entries with priority == 4 (Warning) -> Warning
        if let Some(issue) = create_journal_issue(priority, unit.as_ref(), &entries, config) {
            issues.push(issue);
        }
    }

    // Apply issue correlation if enabled
    if config.enable_correlation {
        correlate_issues(&mut issues, config);
    }

    sort_issues(&mut issues);
    issues
}

/// Create a systemd-related issue from a failed unit.
fn create_systemd_issue(
    unit: &SystemdUnit,
    timestamp: OffsetDateTime,
    config: &AggregationConfig,
) -> Issue {
    let pids = if config.include_pids {
        unit.pids.clone()
    } else {
        Vec::new()
    };

    Issue::new(
        format!("systemd-failed-{}", unit.name),
        Severity::Error,
        format!("Failed Unit: {}", unit.name),
        format!("Systemd unit {} is in failed state", unit.name),
        Some(unit.name.clone()),
        pids,
        timestamp,
        timestamp,
    )
    .with_hints(generate_systemd_hints(&unit.name, config.max_hints))
}

/// Create a mapping from process IDs to systemd units for correlation.
fn create_pid_to_unit_mapping(systemd_units: &[SystemdUnit]) -> HashMap<u32, String> {
    let mut pid_to_unit = HashMap::new();

    for unit in systemd_units {
        for &pid in &unit.pids {
            pid_to_unit.insert(pid, unit.name.clone());
        }
    }

    pid_to_unit
}

/// Group journal entries by priority and unit with enhanced unit mapping.
/// This uses both the `_SYSTEMD_UNIT` field and PID-based correlation.
fn group_journal_entries_enhanced<'a>(
    entries: &'a [JournalEntry],
    pid_to_unit_map: &HashMap<u32, String>,
) -> HashMap<(u8, Option<String>), Vec<&'a JournalEntry>> {
    let mut grouped = HashMap::new();

    for entry in entries {
        // Suppress D-Bus security policy denials generated by our own queries.
        // These flood the journal when the daemon doesn't have permission to call
        // GetProcesses on system units, creating a self-inflicted score penalty.
        if entry
            .unit
            .as_deref()
            .is_some_and(|u| u.starts_with("dbus-broker"))
            && entry.message.contains("security policy denied")
        {
            continue;
        }

        // Determine the unit for this entry using multiple strategies:
        // 1. Use _SYSTEMD_UNIT field if available
        // 2. Map PID to unit using systemd unit information
        // 3. Fall back to no unit mapping
        let mapped_unit = entry
            .unit
            .clone()
            .or_else(|| entry.pid.and_then(|pid| pid_to_unit_map.get(&pid).cloned()))
            .or_else(|| infer_source_key_from_message(&entry.message));

        grouped
            .entry((entry.priority, mapped_unit))
            .or_insert_with(Vec::new)
            .push(entry);
    }

    grouped
}

/// Return true if this entry is a non-actionable one-time boot noise message.
fn is_boot_noise(entry: &JournalEntry) -> bool {
    if entry.unit.is_some() {
        return false;
    }

    BOOT_NOISE_PATTERNS
        .iter()
        .any(|pattern| entry.message.contains(pattern))
}

/// Collect timestamps of clean shutdown/reboot markers in this window.
fn shutdown_marker_times(
    entries: &[JournalEntry],
    config: &AggregationConfig,
) -> Vec<OffsetDateTime> {
    entries
        .iter()
        .filter(|entry| {
            config
                .shutdown_markers
                .iter()
                .any(|marker| entry.message.contains(marker))
        })
        .map(|entry| entry.timestamp)
        .collect()
}

/// Return true if this entry is expected fallout of a clean shutdown/reboot:
/// it matches a transient pattern and falls within the grace window of a
/// detected shutdown marker. Identical messages without a nearby marker
/// (i.e. a real crash) score normally.
fn is_shutdown_transient(
    entry: &JournalEntry,
    marker_times: &[OffsetDateTime],
    config: &AggregationConfig,
) -> bool {
    if config.shutdown_grace_secs <= 0 {
        return false;
    }
    if !config
        .shutdown_transient_patterns
        .iter()
        .any(|pattern| entry.message.contains(pattern))
    {
        return false;
    }
    marker_times.iter().any(|marker| {
        (*marker - entry.timestamp).whole_seconds().abs() <= config.shutdown_grace_secs
    })
}

/// Infer a stable source key from a journal message when unit/PID mapping is unavailable.
///
/// This improves issue attribution for kernel and subsystem warnings that don't include
/// `_SYSTEMD_UNIT`, avoiding a single generic `unknown` bucket.
fn infer_source_key_from_message(message: &str) -> Option<String> {
    let source = message
        .split_once(':')
        .map(|(prefix, _)| prefix.trim())
        .filter(|prefix| !prefix.is_empty())?;

    let mut sanitized = String::new();
    for ch in source.chars() {
        if ch.is_ascii_alphanumeric() {
            sanitized.push(ch.to_ascii_lowercase());
        } else if matches!(ch, '-' | '_' | '.' | '/') {
            sanitized.push('-');
        }
    }

    while sanitized.contains("--") {
        sanitized = sanitized.replace("--", "-");
    }

    let key = sanitized.trim_matches('-');
    if key.is_empty() {
        return None;
    }

    Some(format!("source-{key}"))
}

/// Group journal entries by priority and unit for efficient processing.
#[allow(dead_code)]
fn group_journal_entries(
    entries: &[JournalEntry],
) -> HashMap<(u8, Option<String>), Vec<&JournalEntry>> {
    let mut grouped = HashMap::new();

    for entry in entries {
        grouped
            .entry((entry.priority, entry.unit.clone()))
            .or_insert_with(Vec::new)
            .push(entry);
    }

    grouped
}

/// Create an issue from grouped journal entries if they meet severity criteria.
fn create_journal_issue(
    priority: u8,
    unit: Option<&String>,
    entries: &[&JournalEntry],
    config: &AggregationConfig,
) -> Option<Issue> {
    // Pattern matching with early returns
    let severity = match priority {
        0..=3 => Severity::Error, // Emergency, Alert, Critical, Error
        4 => Severity::Warning,   // Warning
        _ => return None,         // Info, Debug, etc. - ignored
    };

    // Safe unwrapping with error handling
    let first_entry = entries.first()?;
    let last_entry = entries.last()?;

    let pids = if config.include_pids {
        entries.iter().filter_map(|e| e.pid).collect()
    } else {
        Vec::new()
    };

    let id = generate_issue_id(priority, unit, severity);
    let title = match unit {
        Some(u) => format!("{severity} Journal Events — {u}"),
        None => format!("{severity} Journal Events"),
    };
    let summary = format!(
        "Grouped {} {} journal entries",
        entries.len(),
        severity.to_string().to_lowercase()
    );

    let mut issue = Issue::new(
        id,
        severity,
        title,
        summary,
        unit.cloned(),
        pids,
        first_entry.timestamp,
        last_entry.timestamp,
    );

    // Set the actual count and add hints
    issue.count = entries.len();
    issue = issue.with_hints(generate_journal_hints(entries, config.max_hints));

    Some(issue)
}

/// Generate a unique ID for a journal-based issue.
fn generate_issue_id(priority: u8, unit: Option<&String>, severity: Severity) -> String {
    let unit_part = unit.map_or("unknown", |s| s.as_str());
    format!(
        "journal-{}-{}-{}",
        severity.to_string().to_lowercase(),
        priority,
        unit_part
    )
}

/// Generate helpful hints for systemd issues.
fn generate_systemd_hints(unit_name: &str, max_hints: usize) -> Vec<String> {
    let mut hints = vec![
        format!("Check unit status with: systemctl status {unit_name}"),
        format!("View logs with: journalctl -u {unit_name}"),
        format!("Restart unit with: sudo systemctl restart {unit_name}"),
    ];

    hints.truncate(max_hints);
    hints
}

/// Generate helpful hints from journal entries.
fn generate_journal_hints(entries: &[&JournalEntry], max_hints: usize) -> Vec<String> {
    entries
        .iter()
        .take(max_hints)
        .map(|e| {
            format!(
                "{}: {}",
                e.timestamp
                    .format(&time::format_description::well_known::Rfc3339)
                    .unwrap_or_default(),
                e.message
            )
        })
        .collect()
}

/// Apply sophisticated issue correlation to group related problems.
/// This identifies issues that are likely caused by the same underlying problem.
fn correlate_issues(issues: &mut Vec<Issue>, config: &AggregationConfig) {
    if issues.len() < 2 {
        return; // Need at least 2 issues to correlate
    }

    let mut correlation_groups: Vec<Vec<usize>> = Vec::new();
    let mut processed_indices = std::collections::HashSet::new();

    // Find correlation groups
    for i in 0..issues.len() {
        if processed_indices.contains(&i) {
            continue;
        }

        let mut current_group = vec![i];
        processed_indices.insert(i);

        // Compare with remaining issues
        for j in (i + 1)..issues.len() {
            if processed_indices.contains(&j)
                || current_group.len() >= config.max_correlation_group_size
            {
                continue;
            }

            if are_issues_correlated(&issues[i], &issues[j], config.correlation_threshold) {
                current_group.push(j);
                processed_indices.insert(j);
            }
        }

        if current_group.len() > 1 {
            correlation_groups.push(current_group);
        }
    }

    // Merge correlated issues
    merge_correlated_issues(issues, correlation_groups, config);
}

/// Determine if two issues are correlated based on various similarity metrics.
fn are_issues_correlated(issue1: &Issue, issue2: &Issue, threshold: f64) -> bool {
    // Issues of different severities are generally not correlated
    if issue1.severity != issue2.severity {
        return false;
    }

    // Journal-derived buckets are already grouped by (priority, unit/source).
    // Correlating them again creates oversized synthetic issues and over-penalizes
    // bursts of unrelated warnings/errors.
    if is_journal_issue(issue1) || is_journal_issue(issue2) {
        return false;
    }

    // Calculate various similarity scores
    let title_similarity = calculate_string_similarity(&issue1.title, &issue2.title);
    let summary_similarity = calculate_string_similarity(&issue1.summary, &issue2.summary);
    let unit_similarity = calculate_unit_similarity(issue1.unit.as_ref(), issue2.unit.as_ref());
    let time_similarity = calculate_time_similarity(issue1.first_seen, issue2.first_seen);

    // Weighted average of similarity scores
    let overall_similarity = (title_similarity * 0.3)
        + (summary_similarity * 0.4)
        + (unit_similarity * 0.2)
        + (time_similarity * 0.1);

    overall_similarity >= threshold
}

fn is_journal_issue(issue: &Issue) -> bool {
    issue.id.starts_with("journal-") || issue.title.ends_with("Journal Events")
}

/// Calculate string similarity using a simple token-based approach.
#[allow(clippy::cast_precision_loss)]
fn calculate_string_similarity(str1: &str, str2: &str) -> f64 {
    if str1 == str2 {
        return 1.0;
    }

    let tokens1: std::collections::HashSet<&str> = str1.split_whitespace().collect();
    let tokens2: std::collections::HashSet<&str> = str2.split_whitespace().collect();

    if tokens1.is_empty() && tokens2.is_empty() {
        return 1.0;
    }

    let intersection_count = tokens1.intersection(&tokens2).count();
    let union_count = tokens1.union(&tokens2).count();

    if union_count == 0 {
        return 0.0;
    }

    intersection_count as f64 / union_count as f64
}

/// Calculate unit similarity considering unit name relationships.
fn calculate_unit_similarity(unit1: Option<&String>, unit2: Option<&String>) -> f64 {
    match (unit1, unit2) {
        (Some(u1), Some(u2)) => {
            if u1 == u2 {
                1.0
            } else {
                // Check for related units (e.g., service and timer, parent/child services)
                calculate_unit_relationship_similarity(u1, u2)
            }
        }
        (None, None) => 0.5, // Both unknown, some similarity
        _ => 0.0,            // One known, one unknown
    }
}

/// Calculate relationship similarity between different unit names.
#[allow(clippy::cast_precision_loss)]
fn calculate_unit_relationship_similarity(unit1: &str, unit2: &str) -> f64 {
    // Extract base names without extensions
    let base1 = unit1.split('.').next().unwrap_or(unit1);
    let base2 = unit2.split('.').next().unwrap_or(unit2);

    if base1 == base2 {
        return 0.8; // Same base name, high similarity
    }

    // Check for common prefixes (e.g., nginx-proxy, nginx-server)
    let common_prefix_len = base1
        .chars()
        .zip(base2.chars())
        .take_while(|(a, b)| a == b)
        .count();

    if common_prefix_len >= 3 {
        let min_len = base1.len().min(base2.len());
        (common_prefix_len as f64 / min_len as f64) * 0.6
    } else {
        0.0
    }
}

/// Calculate time-based similarity for issues that occurred close together.
fn calculate_time_similarity(time1: time::OffsetDateTime, time2: time::OffsetDateTime) -> f64 {
    let diff = (time1 - time2).abs();
    let minutes = diff.whole_minutes().abs();

    if minutes == 0 {
        1.0
    } else if minutes <= 5 {
        0.8
    } else if minutes <= 15 {
        0.5
    } else if minutes <= 60 {
        0.2
    } else {
        0.0
    }
}

/// Merge correlated issues into combined issues with enhanced information.
fn merge_correlated_issues(
    issues: &mut Vec<Issue>,
    correlation_groups: Vec<Vec<usize>>,
    config: &AggregationConfig,
) {
    let mut merged_issues = Vec::new();
    let mut merged_indices = std::collections::HashSet::new();

    // Process correlation groups
    for group in correlation_groups {
        if group.len() < 2 {
            continue;
        }

        // Create merged issue from the group
        let primary_issue = &issues[group[0]];
        let mut merged_issue = primary_issue.clone();

        // Update merged issue properties
        merged_issue.id = format!(
            "correlated-{}",
            group
                .iter()
                .map(std::string::ToString::to_string)
                .collect::<Vec<_>>()
                .join("-")
        );
        merged_issue.title = format!("Correlated Issues: {}", primary_issue.title);
        merged_issue.summary = format!(
            "Group of {} related issues: {}",
            group.len(),
            group
                .iter()
                .map(|&i| issues[i].title.clone())
                .collect::<Vec<_>>()
                .join(", ")
        );

        // Aggregate counts and find time range
        let mut total_count = 0;
        let mut earliest_time = primary_issue.first_seen;
        let mut latest_time = primary_issue.last_seen;
        let mut all_pids = std::collections::HashSet::new();
        let mut all_hints = Vec::new();

        for &idx in &group {
            let issue = &issues[idx];
            total_count += issue.count;

            if issue.first_seen < earliest_time {
                earliest_time = issue.first_seen;
            }
            if issue.last_seen > latest_time {
                latest_time = issue.last_seen;
            }

            // Collect PIDs
            for &pid in &issue.pids {
                all_pids.insert(pid);
            }

            // Collect hints
            for hint in &issue.hints {
                if !all_hints.contains(hint) {
                    all_hints.push(hint.clone());
                }
            }
        }

        merged_issue.count = total_count;
        merged_issue.first_seen = earliest_time;
        merged_issue.last_seen = latest_time;
        merged_issue.pids = all_pids.into_iter().collect();
        merged_issue.hints = all_hints.into_iter().take(config.max_hints * 2).collect(); // Allow more hints for correlated issues

        merged_issues.push(merged_issue);

        // Mark indices as merged
        for &idx in &group {
            merged_indices.insert(idx);
        }
    }

    // Keep non-merged issues and add merged ones
    let mut final_issues = Vec::new();

    for (idx, issue) in issues.iter().enumerate() {
        if !merged_indices.contains(&idx) {
            final_issues.push(issue.clone());
        }
    }

    final_issues.extend(merged_issues);
    *issues = final_issues;
}

/// Sort issues by priority: severity, then `last_seen` desc, then count desc.
fn sort_issues(issues: &mut [Issue]) {
    issues.sort_by(|a, b| {
        // Primary sort: by severity (Error < Warning < Info due to enum values)
        a.severity
            .cmp(&b.severity)
            // Secondary sort: by last_seen descending (most recent first)
            .then_with(|| b.last_seen.cmp(&a.last_seen))
            // Tertiary sort: by count descending (highest count first)
            .then_with(|| b.count.cmp(&a.count))
    });
}

#[cfg(test)]
mod tests {
    use time::macros::datetime;

    use super::*;
    use crate::data::traits::{JournalEntry, SystemdUnit};

    #[test]
    fn test_failed_unit_creates_error() {
        let units = vec![SystemdUnit {
            name: "failed.service".to_string(),
            active_state: "failed".to_string(),
            load_state: "loaded".to_string(),
            sub_state: "failed".to_string(),
            description: "A failed service".to_string(),
            pids: vec![123],
        }];

        let issues = aggregate_issues(&[], &units);

        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].severity, Severity::Error);
        assert_eq!(issues[0].title, "Failed Unit: failed.service");
    }

    #[test]
    fn test_priority_grouping() {
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
        ];

        let issues = aggregate_issues(&entries, &[]);

        assert_eq!(issues.len(), 2);

        // First should be Error (higher priority)
        assert_eq!(issues[0].severity, Severity::Error);
        assert_eq!(issues[0].count, 2);

        // Second should be Warning
        assert_eq!(issues[1].severity, Severity::Warning);
        assert_eq!(issues[1].count, 1);
    }

    #[test]
    fn test_sorting_order() {
        let entries = vec![
            JournalEntry {
                timestamp: datetime!(2024-09-15 10:00:00 UTC),
                priority: 4, // Warning
                message: "Warning".to_string(),
                unit: Some("a.service".to_string()),
                pid: Some(123),
            },
            JournalEntry {
                timestamp: datetime!(2024-09-15 10:01:00 UTC),
                priority: 3, // Error
                message: "Error".to_string(),
                unit: Some("b.service".to_string()),
                pid: Some(124),
            },
        ];

        let issues = aggregate_issues(&entries, &[]);

        assert_eq!(issues.len(), 2);
        // Error should come before Warning
        assert_eq!(issues[0].severity, Severity::Error);
        assert_eq!(issues[1].severity, Severity::Warning);
    }

    #[test]
    fn test_enhanced_pid_to_unit_mapping() {
        let units = vec![SystemdUnit {
            name: "test.service".to_string(),
            active_state: "active".to_string(),
            load_state: "loaded".to_string(),
            sub_state: "running".to_string(),
            description: "Test service".to_string(),
            pids: vec![1234, 5678],
        }];

        let entries = vec![
            JournalEntry {
                timestamp: datetime!(2024-09-15 10:00:00 UTC),
                priority: 3, // Error
                message: "Error from process".to_string(),
                unit: None,      // No _SYSTEMD_UNIT field
                pid: Some(1234), // Should map to test.service via PID
            },
            JournalEntry {
                timestamp: datetime!(2024-09-15 10:01:00 UTC),
                priority: 3, // Error
                message: "Another error".to_string(),
                unit: Some("other.service".to_string()), // Has _SYSTEMD_UNIT, should use that
                pid: Some(1234),                         // Same PID but unit field takes precedence
            },
        ];

        let config = AggregationConfig {
            enable_correlation: false,
            ..Default::default()
        };

        let issues = aggregate_issues_with_config(&entries, &units, &config);

        // Should create 2 separate issues - one for test.service (PID mapped) and one for other.service
        assert_eq!(issues.len(), 2);

        // Find the issue for test.service (PID-mapped entry)
        let test_service_issue = issues
            .iter()
            .find(|issue| issue.unit.as_ref().is_some_and(|u| u == "test.service"))
            .expect("Should have issue for test.service");
        assert_eq!(test_service_issue.count, 1);

        // Find the issue for other.service (explicit unit field)
        let other_service_issue = issues
            .iter()
            .find(|issue| issue.unit.as_ref().is_some_and(|u| u == "other.service"))
            .expect("Should have issue for other.service");
        assert_eq!(other_service_issue.count, 1);
    }

    #[test]
    fn test_pid_to_unit_mapping_creation() {
        let units = vec![
            SystemdUnit {
                name: "first.service".to_string(),
                active_state: "active".to_string(),
                load_state: "loaded".to_string(),
                sub_state: "running".to_string(),
                description: "First service".to_string(),
                pids: vec![100, 200],
            },
            SystemdUnit {
                name: "second.service".to_string(),
                active_state: "active".to_string(),
                load_state: "loaded".to_string(),
                sub_state: "running".to_string(),
                description: "Second service".to_string(),
                pids: vec![300, 400],
            },
        ];

        let mapping = create_pid_to_unit_mapping(&units);

        assert_eq!(mapping.len(), 4);
        assert_eq!(mapping.get(&100), Some(&"first.service".to_string()));
        assert_eq!(mapping.get(&200), Some(&"first.service".to_string()));
        assert_eq!(mapping.get(&300), Some(&"second.service".to_string()));
        assert_eq!(mapping.get(&400), Some(&"second.service".to_string()));
        assert_eq!(mapping.get(&500), None);
    }

    #[test]
    fn test_issue_correlation_same_unit() {
        let entries = vec![
            JournalEntry {
                timestamp: datetime!(2024-09-15 10:00:00 UTC),
                priority: 3, // Error
                message: "Database connection failed timeout error".to_string(),
                unit: Some("webapp.service".to_string()),
                pid: Some(1001),
            },
            JournalEntry {
                timestamp: datetime!(2024-09-15 10:01:00 UTC),
                priority: 4, // Warning - different priority creates separate issue
                message: "Database connection timeout failed error".to_string(),
                unit: Some("webapp.service".to_string()),
                pid: Some(1002),
            },
        ];

        let config = AggregationConfig {
            enable_correlation: true,
            correlation_threshold: 0.5,
            ..Default::default()
        };

        let issues = aggregate_issues_with_config(&entries, &[], &config);

        // Despite different priorities, they should NOT be correlated (different severity)
        // This test actually verifies that correlation respects severity boundaries
        assert_eq!(issues.len(), 2);
    }

    #[test]
    fn test_issue_correlation_related_units() {
        let entries = vec![
            JournalEntry {
                timestamp: datetime!(2024-09-15 10:00:00 UTC),
                priority: 3,
                message: "Service failed to start".to_string(),
                unit: Some("nginx.service".to_string()),
                pid: Some(1001),
            },
            JournalEntry {
                timestamp: datetime!(2024-09-15 10:01:00 UTC),
                priority: 3,
                message: "Service failed to start".to_string(),
                unit: Some("nginx-proxy.service".to_string()),
                pid: Some(1002),
            },
        ];

        let config = AggregationConfig {
            enable_correlation: true,
            correlation_threshold: 0.5,
            ..Default::default()
        };

        let issues = aggregate_issues_with_config(&entries, &[], &config);

        // Journal buckets are already grouped; they should not be correlated again.
        assert_eq!(issues.len(), 2);
    }

    #[test]
    fn test_systemd_issues_can_still_correlate() {
        let units = vec![
            SystemdUnit {
                name: "nginx.service".to_string(),
                active_state: "failed".to_string(),
                load_state: "loaded".to_string(),
                sub_state: "failed".to_string(),
                description: "Nginx web server".to_string(),
                pids: vec![],
            },
            SystemdUnit {
                name: "nginx.timer".to_string(),
                active_state: "failed".to_string(),
                load_state: "loaded".to_string(),
                sub_state: "failed".to_string(),
                description: "Nginx periodic task".to_string(),
                pids: vec![],
            },
        ];

        let config = AggregationConfig {
            enable_correlation: true,
            correlation_threshold: 0.5,
            ..Default::default()
        };

        let issues = aggregate_issues_with_config(&[], &units, &config);
        assert_eq!(issues.len(), 1);
        assert!(issues[0].title.contains("Correlated Issues"));
    }

    #[test]
    fn test_correlation_disabled() {
        let entries = vec![
            JournalEntry {
                timestamp: datetime!(2024-09-15 10:00:00 UTC),
                priority: 3,
                message: "Connection failed".to_string(),
                unit: Some("webapp.service".to_string()),
                pid: Some(1001),
            },
            JournalEntry {
                timestamp: datetime!(2024-09-15 10:02:00 UTC),
                priority: 3,
                message: "Connection failed".to_string(),
                unit: Some("webapp.service".to_string()),
                pid: Some(1002),
            },
        ];

        let config = AggregationConfig {
            enable_correlation: false,
            ..Default::default()
        };

        let issues = aggregate_issues_with_config(&entries, &[], &config);

        // Should remain separate when correlation is disabled
        assert_eq!(issues.len(), 1); // Still grouped by priority and unit, but not correlated
    }

    #[test]
    fn test_string_similarity_calculation() {
        assert!(
            (calculate_string_similarity("connection failed", "connection failed") - 1.0).abs()
                < f64::EPSILON
        );
        assert!(calculate_string_similarity("connection failed", "failed connection") > 0.8);
        assert!(calculate_string_similarity("connection failed", "database timeout") < 0.5);
        assert!((calculate_string_similarity("", "") - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_unit_relationship_similarity() {
        assert!(
            (calculate_unit_relationship_similarity("nginx.service", "nginx.timer") - 0.8).abs()
                < 0.1
        );
        assert!(
            calculate_unit_relationship_similarity("nginx-proxy.service", "nginx-server.service")
                > 0.3
        );
        assert!(
            calculate_unit_relationship_similarity("apache.service", "nginx.service").abs()
                < f64::EPSILON
        );
    }

    #[test]
    fn test_enhanced_grouping_fallback_to_pid_mapping() {
        let entries = vec![
            JournalEntry {
                timestamp: datetime!(2024-09-15 10:00:00 UTC),
                priority: 3,
                message: "Error without unit".to_string(),
                unit: None,      // No _SYSTEMD_UNIT
                pid: Some(1000), // Should be mapped via PID
            },
            JournalEntry {
                timestamp: datetime!(2024-09-15 10:01:00 UTC),
                priority: 3,
                message: "Error with unit".to_string(),
                unit: Some("explicit.service".to_string()), // Has _SYSTEMD_UNIT
                pid: Some(2000),
            },
        ];

        let pid_to_unit_map = HashMap::from([
            (1000, "mapped.service".to_string()),
            (2000, "should-not-be-used.service".to_string()),
        ]);

        let grouped = group_journal_entries_enhanced(&entries, &pid_to_unit_map);

        // Should have 2 groups
        assert_eq!(grouped.len(), 2);

        // First entry should be grouped under mapped.service (from PID mapping)
        let mapped_key = (3, Some("mapped.service".to_string()));
        assert!(grouped.contains_key(&mapped_key));
        assert_eq!(grouped.get(&mapped_key).unwrap().len(), 1);

        // Second entry should be grouped under explicit.service (from unit field)
        let explicit_key = (3, Some("explicit.service".to_string()));
        assert!(grouped.contains_key(&explicit_key));
        assert_eq!(grouped.get(&explicit_key).unwrap().len(), 1);
    }

    #[test]
    fn test_unknown_unit_entries_grouped_by_inferred_source() {
        let entries = vec![
            JournalEntry {
                timestamp: datetime!(2024-09-15 10:00:00 UTC),
                priority: 4,
                message: "zfs: unknown parameter '#' ignored".to_string(),
                unit: None,
                pid: None,
            },
            JournalEntry {
                timestamp: datetime!(2024-09-15 10:01:00 UTC),
                priority: 4,
                message: "IPv4: martian destination 0.0.0.0 from 192.168.1.2".to_string(),
                unit: None,
                pid: None,
            },
        ];

        let config = AggregationConfig {
            enable_correlation: false,
            ..Default::default()
        };

        let issues = aggregate_issues_with_config(&entries, &[], &config);
        assert_eq!(issues.len(), 2);
        assert!(issues
            .iter()
            .any(|issue| issue.unit.as_deref() == Some("source-zfs")));
        assert!(issues
            .iter()
            .any(|issue| issue.unit.as_deref() == Some("source-ipv4")));
    }

    #[test]
    fn test_boot_noise_is_filtered_from_issues() {
        let entries = vec![
            JournalEntry {
                timestamp: datetime!(2024-09-15 10:00:00 UTC),
                priority: 4,
                message: "MDS CPU bug present and SMT on, data leak possible".to_string(),
                unit: None,
                pid: None,
            },
            JournalEntry {
                timestamp: datetime!(2024-09-15 10:01:00 UTC),
                priority: 4,
                message: "zfs: module license 'CDDL' taints kernel.".to_string(),
                unit: None,
                pid: None,
            },
            JournalEntry {
                timestamp: datetime!(2024-09-15 10:02:00 UTC),
                priority: 4,
                message: "service warning should remain".to_string(),
                unit: Some("example.service".to_string()),
                pid: Some(123),
            },
        ];

        let config = AggregationConfig {
            enable_correlation: false,
            ..Default::default()
        };

        let issues = aggregate_issues_with_config(&entries, &[], &config);
        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].unit.as_deref(), Some("example.service"));
        assert_eq!(issues[0].count, 1);
    }

    /// Shutdown fallout clustered around a reboot marker must not score.
    /// Mirrors the m920q report: samba SIGTERM, resolved degraded-UDP, and
    /// Redis exit notices all sharing one shutdown second.
    #[test]
    fn test_shutdown_transients_suppressed_near_marker() {
        let entries = vec![
            JournalEntry {
                timestamp: datetime!(2026-09-04 07:32:39 UTC),
                priority: 3,
                message: "Got sig[15] terminate (is_parent=0)".to_string(),
                unit: Some("samba-winbindd.service".to_string()),
                pid: Some(101),
            },
            JournalEntry {
                timestamp: datetime!(2026-09-04 07:32:39 UTC),
                priority: 4,
                message: "Using degraded feature set UDP instead of TCP for DNS server 127.0.0.1."
                    .to_string(),
                unit: Some("systemd-resolved.service".to_string()),
                pid: Some(102),
            },
            JournalEntry {
                timestamp: datetime!(2026-09-04 07:32:39 UTC),
                priority: 4,
                message: "Redis is now ready to exit, bye bye...".to_string(),
                unit: Some("redis-immich.service".to_string()),
                pid: Some(103),
            },
            JournalEntry {
                timestamp: datetime!(2026-09-04 07:32:40 UTC),
                priority: 6,
                message: "Reached target System Shutdown.".to_string(),
                unit: None,
                pid: None,
            },
        ];

        let config = AggregationConfig {
            enable_correlation: false,
            ..Default::default()
        };

        let issues = aggregate_issues_with_config(&entries, &[], &config);
        assert!(issues.is_empty());
    }

    /// Identical messages without any shutdown marker are real failures and
    /// must keep scoring.
    #[test]
    fn test_shutdown_transients_kept_without_marker() {
        let entries = vec![
            JournalEntry {
                timestamp: datetime!(2026-09-04 07:32:39 UTC),
                priority: 3,
                message: "Got sig[15] terminate (is_parent=0)".to_string(),
                unit: Some("samba-winbindd.service".to_string()),
                pid: Some(101),
            },
            JournalEntry {
                timestamp: datetime!(2026-09-04 07:32:39 UTC),
                priority: 4,
                message: "Using degraded feature set UDP instead of TCP for DNS server 127.0.0.1."
                    .to_string(),
                unit: Some("systemd-resolved.service".to_string()),
                pid: Some(102),
            },
        ];

        let config = AggregationConfig {
            enable_correlation: false,
            ..Default::default()
        };

        let issues = aggregate_issues_with_config(&entries, &[], &config);
        assert_eq!(issues.len(), 2);
        assert!(issues.iter().any(|issue| issue.severity == Severity::Error));
    }

    /// An exit-code failure far outside the grace window is not shutdown
    /// fallout, even when a marker exists elsewhere in the window.
    #[test]
    fn test_exit_code_kept_outside_grace_window() {
        let entries = vec![
            JournalEntry {
                timestamp: datetime!(2026-09-04 07:00:00 UTC),
                priority: 6,
                message: "Reached target System Shutdown.".to_string(),
                unit: None,
                pid: None,
            },
            JournalEntry {
                timestamp: datetime!(2026-09-04 12:00:00 UTC),
                priority: 3,
                message: "homepage-dashboard.service: Failed with result 'exit-code'.".to_string(),
                unit: Some("homepage-dashboard.service".to_string()),
                pid: Some(104),
            },
        ];

        let config = AggregationConfig {
            enable_correlation: false,
            ..Default::default()
        };

        let issues = aggregate_issues_with_config(&entries, &[], &config);
        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].severity, Severity::Error);
    }

    /// Zero grace disables the suppression entirely.
    #[test]
    fn test_shutdown_suppression_disabled_by_zero_grace() {
        let entries = vec![
            JournalEntry {
                timestamp: datetime!(2026-09-04 07:32:39 UTC),
                priority: 3,
                message: "Got sig[15] terminate (is_parent=0)".to_string(),
                unit: Some("samba-winbindd.service".to_string()),
                pid: Some(101),
            },
            JournalEntry {
                timestamp: datetime!(2026-09-04 07:32:40 UTC),
                priority: 6,
                message: "Reached target System Shutdown.".to_string(),
                unit: None,
                pid: None,
            },
        ];

        let config = AggregationConfig {
            enable_correlation: false,
            shutdown_grace_secs: 0,
            ..Default::default()
        };

        let issues = aggregate_issues_with_config(&entries, &[], &config);
        assert_eq!(issues.len(), 1);
    }
}
