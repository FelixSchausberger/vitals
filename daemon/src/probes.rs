//! Active health probes.
//!
//! These probes detect silent failure modes that the journal aggregator misses:
//!
//! | Probe           | Detection method                                             |
//! |-----------------|--------------------------------------------------------------|
//! | OOM kill        | Scan journal entries for kernel OOM kill messages            |
//! | Restart loop    | Count scheduled restarts per unit across ticks; a         |
//! |                 | start-limit-hit escalates the issue to Error                  |
//! | Boot anomaly    | Run `systemd-analyze blame` once at startup; flag slow units |
//! | RAM trend       | Track per-unit RSS over time; flag monotonic growth          |
//!
//! Each probe returns `Vec<Issue>` that is merged with the aggregated issue list
//! before being fed to the TWHS calculator.

use std::collections::{HashMap, HashSet, VecDeque};

use time::OffsetDateTime;

use crate::{
    data::traits::{JournalEntry, UnitMetrics},
    model::{Issue, Severity},
};

// ── Constants ─────────────────────────────────────────────────────────────────

/// Units slower than this at boot are flagged as a warning.
const BOOT_SLOW_THRESHOLD_SECS: f64 = 30.0;

/// Number of consecutive rising RSS samples required to flag a RAM trend.
const RAM_TREND_MIN_SAMPLES: usize = 5;

/// Number of per-unit RSS samples to keep in the rolling buffer.
const RAM_TREND_BUFFER_SIZE: usize = 20;

/// Restart messages within this window make a unit a restart loop.
const RESTART_LOOP_WINDOW_SECS: i64 = 600;

/// Number of restart messages in the window required to flag a loop.
const RESTART_LOOP_THRESHOLD: usize = 5;

// ── ProbeState ────────────────────────────────────────────────────────────────

/// Mutable state held by the probe collector across polling ticks.
pub struct ProbeState {
    /// Per-unit RSS history (bytes).
    rss_history: HashMap<String, VecDeque<u64>>,
    /// Per-unit restart history for loop detection.
    restart_loops: RestartLoopDetector,
    /// Boot anomaly issues — computed once at startup, re-emitted every tick
    /// (with original `first_seen` so temporal decay handles them correctly).
    boot_issues: Vec<Issue>,
}

#[allow(clippy::new_without_default)]
impl ProbeState {
    /// Create a new probe state and run the one-time boot anomaly scan.
    #[must_use]
    pub fn new() -> Self {
        let boot_issues = detect_boot_anomalies();
        Self {
            rss_history: HashMap::new(),
            restart_loops: RestartLoopDetector::default(),
            boot_issues,
        }
    }

    /// Collect synthetic issues from all active probes.
    ///
    /// Called once per polling tick with the freshly-fetched raw data.
    pub fn collect(
        &mut self,
        journal_entries: &[JournalEntry],
        unit_metrics: Option<&[UnitMetrics]>,
        now: OffsetDateTime,
    ) -> Vec<Issue> {
        let mut issues = self.boot_issues.clone();
        issues.extend(scan_oom_kills(journal_entries, now));
        issues.extend(self.restart_loops.collect(journal_entries, now));
        if let Some(metrics) = unit_metrics {
            issues.extend(self.detect_ram_trends(metrics, now));
        }
        issues
    }

    // ── RAM trend ─────────────────────────────────────────────────────────────

    fn detect_ram_trends(
        &mut self,
        unit_metrics: &[UnitMetrics],
        now: OffsetDateTime,
    ) -> Vec<Issue> {
        let mut issues = Vec::new();

        for unit in unit_metrics {
            if unit.memory_rss == 0 {
                continue;
            }

            let history = self
                .rss_history
                .entry(unit.unit_name.clone())
                .or_insert_with(|| VecDeque::with_capacity(RAM_TREND_BUFFER_SIZE));

            if history.len() == RAM_TREND_BUFFER_SIZE {
                history.pop_front();
            }
            history.push_back(unit.memory_rss);

            if history.len() < RAM_TREND_MIN_SAMPLES {
                continue;
            }

            // Check if RSS has grown on every consecutive sample pair
            let monotonically_rising = history
                .iter()
                .zip(history.iter().skip(1))
                .all(|(a, b)| b > a);

            if monotonically_rising {
                let oldest_mb = history.front().copied().unwrap_or(0) / (1024 * 1024);
                let newest_mb = history.back().copied().unwrap_or(0) / (1024 * 1024);
                issues.push(Issue::new(
                    format!("ram-trend:{}", unit.unit_name),
                    Severity::Warning,
                    format!("RAM growth: {}", unit.unit_name),
                    format!(
                        "RSS has grown monotonically over the last {} samples: {}→{} MB",
                        history.len(),
                        oldest_mb,
                        newest_mb,
                    ),
                    Some(unit.unit_name.clone()),
                    unit.pids.clone(),
                    now,
                    now,
                ));
            }
        }

        issues
    }
}

// ── OOM kill scan ─────────────────────────────────────────────────────────────

/// Scan journal entries for kernel OOM kill messages.
fn scan_oom_kills(entries: &[JournalEntry], _now: OffsetDateTime) -> Vec<Issue> {
    let mut kills: HashMap<String, (usize, OffsetDateTime, OffsetDateTime)> = HashMap::new();

    for entry in entries {
        let msg_lower = entry.message.to_ascii_lowercase();
        if !msg_lower.contains("out of memory")
            && !msg_lower.contains("oom_kill")
            && !msg_lower.contains("killed process")
        {
            continue;
        }

        // Try to extract the killed process name from "Killed process N (name)"
        let comm = extract_oom_comm(&entry.message).unwrap_or_else(|| "unknown".to_string());
        let id = format!("oom:{comm}");

        let (count, first, last) = kills
            .entry(id)
            .or_insert((0, entry.timestamp, entry.timestamp));
        *count += 1;
        if entry.timestamp < *first {
            *first = entry.timestamp;
        }
        if entry.timestamp > *last {
            *last = entry.timestamp;
        }
    }

    kills
        .into_iter()
        .map(|(id, (count, first_seen, last_seen))| {
            let comm = id.strip_prefix("oom:").unwrap_or("unknown");
            let mut issue = Issue::new(
                id.clone(),
                Severity::Error,
                format!("OOM kill: {comm}"),
                format!("Process '{comm}' was killed by the kernel OOM killer ({count} time(s))"),
                None,
                vec![],
                first_seen,
                last_seen,
            );
            issue.count = count;
            issue
        })
        .collect()
}

/// Extract the process name from a "Killed process N (name)" message.
fn extract_oom_comm(message: &str) -> Option<String> {
    // Pattern: "Killed process 1234 (nginx)" or "oom_kill_process: ... comm=nginx"
    if let Some(paren_start) = message.find('(') {
        if let Some(paren_end) = message[paren_start..].find(')') {
            let comm = &message[paren_start + 1..paren_start + paren_end];
            if !comm.is_empty() && !comm.contains(' ') {
                return Some(comm.to_string());
            }
        }
    }
    // Try comm= pattern
    message
        .split_whitespace()
        .find(|s| s.starts_with("comm="))
        .map(|s| s.trim_start_matches("comm=").to_string())
}

// ── Restart loop ──────────────────────────────────────────────────────────────

/// Tracks restart messages across ticks and flags units that keep restarting.
///
/// systemd only logs `start-limit-hit` when a unit exceeds its start rate
/// limit (default: 5 starts in 10 s). A unit whose `RestartSec` spaces attempts
/// outside that window never produces the marker — the media kiosk browser
/// reached 2 700 starts that way — so the scheduled-restart messages are
/// counted instead. A start-limit-hit escalates the finding from a Warning to
/// an Error.
#[derive(Default)]
struct RestartLoopDetector {
    /// Timestamps of restart messages, oldest first, per unit.
    restarts: HashMap<String, VecDeque<OffsetDateTime>>,
}

impl RestartLoopDetector {
    /// Record this tick's restart messages and emit the current loops.
    fn collect(&mut self, entries: &[JournalEntry], now: OffsetDateTime) -> Vec<Issue> {
        let mut hit_limit: HashSet<String> = HashSet::new();

        for entry in entries {
            let Some(unit) = restart_unit(entry) else {
                continue;
            };
            if is_start_limit_message(&entry.message) {
                hit_limit.insert(unit.clone());
            }
            self.restarts
                .entry(unit)
                .or_default()
                .push_back(entry.timestamp);
        }

        let cutoff = now - time::Duration::seconds(RESTART_LOOP_WINDOW_SECS);
        // Journal batches arrive newest-first; sort before pruning so the
        // window and the reported first/last timestamps are chronological.
        for times in self.restarts.values_mut() {
            times.make_contiguous().sort_unstable();
            while times.front().is_some_and(|timestamp| *timestamp < cutoff) {
                times.pop_front();
            }
        }
        self.restarts.retain(|_, times| !times.is_empty());

        let mut candidates: HashSet<&String> = hit_limit.iter().collect();
        candidates.extend(
            self.restarts
                .iter()
                .filter(|(_, times)| times.len() >= RESTART_LOOP_THRESHOLD)
                .map(|(unit, _)| unit),
        );

        candidates
            .into_iter()
            .map(|unit| {
                let history = self.restarts.get(unit);
                let (count, first_seen, last_seen) = match history {
                    Some(times) if !times.is_empty() => (
                        times.len(),
                        *times.front().unwrap(),
                        *times.back().unwrap(),
                    ),
                    _ => (1, now, now),
                };

                let (id, title, severity, description) = if hit_limit.contains(unit) {
                    (
                        format!("restart-storm:{unit}"),
                        format!("Restart storm: {unit}"),
                        Severity::Error,
                        format!(
                            "{unit} hit its start rate limit after {count} restart(s) in the last {} minutes.",
                            RESTART_LOOP_WINDOW_SECS / 60,
                        ),
                    )
                } else {
                    (
                        format!("restart-loop:{unit}"),
                        format!("Restart loop: {unit}"),
                        Severity::Warning,
                        format!(
                            "{unit} restarted {count} times in the last {} minutes and is still looping.",
                            RESTART_LOOP_WINDOW_SECS / 60,
                        ),
                    )
                };

                let mut issue = Issue::new(
                    id,
                    severity,
                    title,
                    description,
                    Some(unit.clone()),
                    vec![],
                    first_seen,
                    last_seen,
                );
                issue.count = count;
                issue
            })
            .collect()
    }
}

/// Extract the restarted unit from a restart message, if the entry is one.
///
/// systemd user-manager records carry `user@1000.service` in the unit field
/// while the message names the service that actually restarted, so a unit name
/// in the message prefix wins over the entry's unit.
fn restart_unit(entry: &JournalEntry) -> Option<String> {
    let message = &entry.message;
    let lower = message.to_ascii_lowercase();
    if !lower.contains("scheduled restart job")
        && !lower.contains("start request repeated")
        && !lower.contains("start-limit-hit")
    {
        return None;
    }

    if let Some(prefix) = message.split(':').next() {
        let candidate = prefix.trim();
        if is_unit_name(candidate) {
            return Some(candidate.to_string());
        }
    }
    entry.unit.clone()
}

/// True when systemd reported that the unit exhausted its start rate limit.
fn is_start_limit_message(message: &str) -> bool {
    let lower = message.to_ascii_lowercase();
    lower.contains("start request repeated") || lower.contains("start-limit-hit")
}

/// True when the string looks like a systemd unit name.
fn is_unit_name(value: &str) -> bool {
    [".service", ".timer", ".socket", ".scope", ".mount", ".path"]
        .iter()
        .any(|suffix| value.ends_with(suffix))
}

// ── Boot anomaly ──────────────────────────────────────────────────────────────

/// Run `systemd-analyze blame` once and return issues for slow units.
///
/// This is called at daemon startup; results are cached in `ProbeState` and
/// re-emitted on every tick (with the original `first_seen` timestamp, so the
/// TWHS temporal decay reduces their impact as the boot recedes in time).
fn detect_boot_anomalies() -> Vec<Issue> {
    let output = match std::process::Command::new("systemd-analyze")
        .arg("blame")
        .output()
    {
        Ok(o) => o,
        Err(e) => {
            eprintln!("probe: systemd-analyze blame unavailable: {e}");
            return Vec::new();
        }
    };

    if !output.status.success() {
        return Vec::new();
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    // Approximate boot time: use current time minus uptime so decay works correctly.
    let boot_time = estimate_boot_time();
    let mut issues = Vec::new();

    for line in stdout.lines() {
        // Lines look like: "  1min 11.234s systemd-tmpfiles-clean.service"
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Some((duration_secs, unit_name)) = parse_blame_line(line) else {
            continue;
        };
        if duration_secs < BOOT_SLOW_THRESHOLD_SECS {
            continue;
        }
        let unit_name = unit_name.to_string();
        let id = format!("boot-slow:{unit_name}");

        issues.push(Issue::new(
            id,
            Severity::Warning,
            format!("Slow boot: {unit_name}"),
            format!(
                "{unit_name} added {duration_secs:.1}s to boot time (threshold: {BOOT_SLOW_THRESHOLD_SECS:.0}s)"
            ),
            Some(unit_name),
            vec![],
            boot_time,
            boot_time,
        ));
    }

    issues
}

/// Parse a `systemd-analyze blame` output line.
///
/// Returns `(duration_secs, unit_name)` or `None` if the line cannot be parsed.
fn parse_blame_line(line: &str) -> Option<(f64, &str)> {
    // Formats seen in the wild:
    //   "1min 11.234s systemd-tmpfiles-clean.service"
    //   "11.234s      nginx.service"
    //   "234ms        something.service"

    let mut parts = line.splitn(2, |c: char| c.is_ascii_alphabetic());
    let _ = parts.next()?; // skip leading spaces already trimmed

    // Find the unit name: last whitespace-separated token
    let tokens: Vec<&str> = line.split_whitespace().collect();
    let unit_name = tokens.last()?;
    if !unit_name.contains('.') {
        return None;
    }

    // Parse duration tokens (everything except the last token)
    let duration_tokens = &tokens[..tokens.len() - 1];
    let mut total_secs = 0.0_f64;

    let mut i = 0;
    while i < duration_tokens.len() {
        let tok = duration_tokens[i];
        // Tokens look like "1min", "11.234s", "234ms"
        if tok.ends_with("min") {
            if let Ok(v) = tok.trim_end_matches("min").parse::<f64>() {
                total_secs += v * 60.0;
            }
        } else if tok.ends_with("ms") {
            if let Ok(v) = tok.trim_end_matches("ms").parse::<f64>() {
                total_secs += v / 1000.0;
            }
        } else if tok.ends_with('s') {
            if let Ok(v) = tok.trim_end_matches('s').parse::<f64>() {
                total_secs += v;
            }
        }
        i += 1;
    }

    if total_secs == 0.0 {
        None
    } else {
        Some((total_secs, unit_name))
    }
}

/// Estimate system boot time from uptime.
fn estimate_boot_time() -> OffsetDateTime {
    let secs = crate::data::metrics_system::uptime_secs().unwrap_or(0);
    #[allow(clippy::cast_possible_wrap)]
    let secs = secs as i64;
    OffsetDateTime::now_utc() - time::Duration::seconds(secs)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(unit: Option<&str>, message: &str, timestamp: OffsetDateTime) -> JournalEntry {
        JournalEntry {
            timestamp,
            priority: 4,
            message: message.to_string(),
            unit: unit.map(str::to_string),
            pid: None,
        }
    }

    fn restart(unit: &str, counter: u32, timestamp: OffsetDateTime) -> JournalEntry {
        entry(
            Some(unit),
            &format!("{unit}: Scheduled restart job, restart counter is at {counter}."),
            timestamp,
        )
    }

    #[test]
    fn restart_loop_is_detected_without_start_limit() {
        let base = OffsetDateTime::now_utc();
        let mut detector = RestartLoopDetector::default();
        let entries: Vec<JournalEntry> = (1..=6)
            .map(|n| {
                restart(
                    "kiosk.service",
                    n,
                    base + time::Duration::seconds(i64::from(n) * 10),
                )
            })
            .collect();

        let issues = detector.collect(&entries, base + time::Duration::seconds(90));

        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].id, "restart-loop:kiosk.service");
        assert_eq!(issues[0].severity, Severity::Warning);
        assert_eq!(issues[0].count, 6);
        assert_eq!(issues[0].unit.as_deref(), Some("kiosk.service"));
    }

    #[test]
    fn four_restarts_stay_below_the_threshold() {
        let base = OffsetDateTime::now_utc();
        let mut detector = RestartLoopDetector::default();
        let entries: Vec<JournalEntry> = (1..=4)
            .map(|n| {
                restart(
                    "kiosk.service",
                    n,
                    base + time::Duration::seconds(i64::from(n) * 10),
                )
            })
            .collect();

        assert!(detector
            .collect(&entries, base + time::Duration::seconds(60))
            .is_empty());
    }

    #[test]
    fn start_limit_escalates_the_issue_to_error() {
        let base = OffsetDateTime::now_utc();
        let mut detector = RestartLoopDetector::default();
        let mut entries: Vec<JournalEntry> = (1..=5)
            .map(|n| {
                restart(
                    "crash.service",
                    n,
                    base + time::Duration::seconds(i64::from(n) * 10),
                )
            })
            .collect();
        entries.push(entry(
            Some("crash.service"),
            "crash.service: Failed with result 'start-limit-hit'.",
            base + time::Duration::seconds(60),
        ));

        let issues = detector.collect(&entries, base + time::Duration::seconds(60));

        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].id, "restart-storm:crash.service");
        assert_eq!(issues[0].severity, Severity::Error);
    }

    #[test]
    fn user_unit_name_comes_from_the_message_prefix() {
        let base = OffsetDateTime::now_utc();
        let mut detector = RestartLoopDetector::default();
        let entries: Vec<JournalEntry> = (1..=5)
            .map(|n| {
                entry(
                    Some("user@1000.service"),
                    &format!(
                        "media-kiosk-browser.service: Scheduled restart job, restart counter is at {n}."
                    ),
                    base + time::Duration::seconds(i64::from(n) * 10),
                )
            })
            .collect();

        let issues = detector.collect(&entries, base + time::Duration::seconds(60));

        assert_eq!(issues.len(), 1);
        assert_eq!(
            issues[0].unit.as_deref(),
            Some("media-kiosk-browser.service")
        );
    }

    #[test]
    fn history_is_pruned_outside_the_window() {
        let base = OffsetDateTime::now_utc();
        let mut detector = RestartLoopDetector::default();
        let entries: Vec<JournalEntry> = (1..=4)
            .map(|n| {
                restart(
                    "kiosk.service",
                    n,
                    base + time::Duration::seconds(i64::from(n) * 10),
                )
            })
            .collect();

        assert!(detector
            .collect(&entries, base + time::Duration::seconds(60))
            .is_empty());
        assert!(detector
            .collect(&[], base + time::Duration::seconds(700))
            .is_empty());

        let later = base + time::Duration::seconds(900);
        let issues = detector.collect(&[restart("kiosk.service", 6, later)], later);
        assert!(
            issues.is_empty(),
            "one restart after the window must not flag"
        );
    }

    #[test]
    fn newest_first_batches_report_chronological_bounds() {
        let base = OffsetDateTime::now_utc();
        let mut detector = RestartLoopDetector::default();
        // The journal reader returns batches newest-first.
        let mut entries: Vec<JournalEntry> = (1..=6)
            .map(|n| {
                restart(
                    "kiosk.service",
                    n,
                    base + time::Duration::seconds(i64::from(n) * 10),
                )
            })
            .collect();
        entries.reverse();

        let issues = detector.collect(&entries, base + time::Duration::seconds(90));

        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].count, 6);
        assert!(
            issues[0].first_seen < issues[0].last_seen,
            "bounds must stay chronological: {} .. {}",
            issues[0].first_seen,
            issues[0].last_seen
        );
    }

    #[test]
    fn non_restart_messages_are_ignored() {
        let base = OffsetDateTime::now_utc();
        let mut detector = RestartLoopDetector::default();
        let entries: Vec<JournalEntry> = (1..=10)
            .map(|n| {
                entry(
                    Some("app.service"),
                    "app.service: Failed to start",
                    base + time::Duration::seconds(i64::from(n)),
                )
            })
            .collect();

        assert!(detector
            .collect(&entries, base + time::Duration::seconds(20))
            .is_empty());
    }
}
