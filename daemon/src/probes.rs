//! Active health probes.
//!
//! These probes detect silent failure modes that the journal aggregator misses:
//!
//! | Probe           | Detection method                                             |
//! |-----------------|--------------------------------------------------------------|
//! | OOM kill        | Scan journal entries for kernel OOM kill messages            |
//! | Restart storm   | Scan journal entries for systemd start-limit-hit entries     |
//! | Boot anomaly    | Run `systemd-analyze blame` once at startup; flag slow units |
//! | RAM trend       | Track per-unit RSS over time; flag monotonic growth          |
//!
//! Each probe returns `Vec<Issue>` that is merged with the aggregated issue list
//! before being fed to the TWHS calculator.

use std::collections::{HashMap, VecDeque};

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

// ── ProbeState ────────────────────────────────────────────────────────────────

/// Mutable state held by the probe collector across polling ticks.
pub struct ProbeState {
    /// Per-unit RSS history (bytes).
    rss_history: HashMap<String, VecDeque<u64>>,
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
        issues.extend(detect_restart_storms(journal_entries, now));
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

// ── Restart storm ─────────────────────────────────────────────────────────────

/// Detect services that hit systemd's start limit.
///
/// Looks for systemd messages of the form:
/// - `<unit>: Start request repeated too quickly`
/// - `<unit>: Failed with result 'start-limit-hit'`
fn detect_restart_storms(entries: &[JournalEntry], _now: OffsetDateTime) -> Vec<Issue> {
    let mut storms: HashMap<String, (usize, OffsetDateTime, OffsetDateTime)> = HashMap::new();

    for entry in entries {
        let msg = &entry.message;
        if !msg.contains("start request repeated too quickly")
            && !msg.contains("start-limit-hit")
            && !msg.contains("Start request repeated")
        {
            continue;
        }

        // Extract unit name: "nginx.service: Start request repeated..." → "nginx.service"
        let unit = entry
            .unit
            .clone()
            .or_else(|| {
                msg.split(':')
                    .next()
                    .map(str::trim)
                    .filter(|s| {
                        let path = std::path::Path::new(s);
                        path.extension()
                            .is_some_and(|e| e.eq_ignore_ascii_case("service"))
                            || path
                                .extension()
                                .is_some_and(|e| e.eq_ignore_ascii_case("timer"))
                            || path
                                .extension()
                                .is_some_and(|e| e.eq_ignore_ascii_case("socket"))
                    })
                    .map(str::to_string)
            })
            .unwrap_or_else(|| "unknown".to_string());

        let id = format!("restart-storm:{unit}");
        let (count, first, last) =
            storms
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

    storms
        .into_iter()
        .map(|(id, (count, first_seen, last_seen))| {
            let unit = id.strip_prefix("restart-storm:").unwrap_or("unknown");
            let mut issue = Issue::new(
                id.clone(),
                Severity::Error,
                format!("Restart storm: {unit}"),
                format!(
                    "{unit} hit its start rate limit ({count} time(s)). \
                     The service is restarting too quickly — check for a crash loop.",
                ),
                Some(unit.to_string()),
                vec![],
                first_seen,
                last_seen,
            );
            issue.count = count;
            issue
        })
        .collect()
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
