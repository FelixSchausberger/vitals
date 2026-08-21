//! Application state and event handling for vitals TUI.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::widgets::ListState;
use time::OffsetDateTime;
use vitals_core::api::{HealthResponse, LogsQuery, LogsResponse, SeverityFilter};

/// Time-window presets for the logs view.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum TimePreset {
    /// The daemon's full configured window (no `since` constraint).
    #[default]
    Window,
    /// Last hour.
    Hour1,
    /// Last six hours.
    Hour6,
}

impl TimePreset {
    /// Cycle to the next preset.
    #[must_use]
    pub const fn next(self) -> Self {
        match self {
            Self::Window => Self::Hour1,
            Self::Hour1 => Self::Hour6,
            Self::Hour6 => Self::Window,
        }
    }

    /// Hours back from now, or `None` for the full window.
    #[must_use]
    pub const fn hours(self) -> Option<i64> {
        match self {
            Self::Window => None,
            Self::Hour1 => Some(1),
            Self::Hour6 => Some(6),
        }
    }

    /// Short label for the filter bar.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Window => "all",
            Self::Hour1 => "1h",
            Self::Hour6 => "6h",
        }
    }
}

/// Active log filters, editable from the logs view.
#[derive(Debug, Clone)]
pub struct LogFilters {
    pub severity: Option<SeverityFilter>,
    pub unit: Option<String>,
    pub time: TimePreset,
    /// Entries requested per page.
    pub page_size: usize,
    /// Pagination offset into the filtered result set.
    pub offset: usize,
}

impl Default for LogFilters {
    fn default() -> Self {
        Self {
            severity: None,
            unit: None,
            time: TimePreset::Window,
            page_size: 200,
            offset: 0,
        }
    }
}

impl LogFilters {
    /// Convert to an API query; `now` anchors relative time presets.
    #[must_use]
    pub fn to_query(&self, now: OffsetDateTime) -> LogsQuery {
        LogsQuery {
            severity: self.severity,
            unit: self.unit.clone(),
            since: self.time.hours().map(|h| {
                #[allow(clippy::cast_possible_wrap)]
                let d = time::Duration::hours(h);
                now - d
            }),
            until: None,
            limit: Some(self.page_size),
            offset: self.offset,
        }
    }

    fn reset(&mut self) {
        self.severity = None;
        self.unit = None;
        self.time = TimePreset::Window;
        self.offset = 0;
    }
}

/// Top-level views available in the TUI.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ViewMode {
    /// Compact overview: health score, issues, metrics bar.
    #[default]
    Summary,
    /// Expanded detail: raw/smoothed scores, resources, consumers, all issues.
    Detailed,
    /// Journal entries from the daemon.
    Logs,
}

impl ViewMode {
    /// All modes in display order.
    pub const ALL: [Self; 3] = [Self::Summary, Self::Detailed, Self::Logs];

    /// Next mode in cycle order.
    #[must_use]
    pub const fn next(self) -> Self {
        match self {
            Self::Summary => Self::Detailed,
            Self::Detailed => Self::Logs,
            Self::Logs => Self::Summary,
        }
    }

    /// Previous mode in cycle order.
    #[must_use]
    pub const fn previous(self) -> Self {
        match self {
            Self::Summary => Self::Logs,
            Self::Detailed => Self::Summary,
            Self::Logs => Self::Detailed,
        }
    }

    /// Short label for the tab bar.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Summary => "Summary",
            Self::Detailed => "Detailed",
            Self::Logs => "Logs",
        }
    }
}

/// What a drill-down pane is showing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DrillDown {
    /// Expanded issue details; `unit` is extracted from the title when possible.
    Issue { index: usize, unit: Option<String> },
    /// Full text of one log entry.
    Log { index: usize },
}

/// Open drill-down pane plus its scroll offset.
#[derive(Debug)]
pub struct DetailState {
    pub kind: DrillDown,
    pub scroll: u16,
}

/// Active regex search over the logs view.
#[derive(Debug, Default)]
pub struct SearchState {
    /// Pattern being typed (or last committed pattern).
    pub input: String,
    /// Compiled pattern; `None` while typing or after an edit.
    pub regex: Option<regex::Regex>,
    /// Indices into the current log entries that match.
    pub matches: Vec<usize>,
    /// Position within `matches`.
    pub current: usize,
    /// Set when the last compile failed, with the error message.
    pub error: Option<String>,
}

impl SearchState {
    #[must_use]
    pub fn is_committed(&self) -> bool {
        self.regex.is_some()
    }
}

/// Mutable application state shared by the event loop and renderer.
#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Default)]
pub struct App {
    pub mode: ViewMode,
    pub should_quit: bool,
    pub show_help: bool,
    pub health: Option<HealthResponse>,
    pub logs: Option<LogsResponse>,
    pub issue_selection: ListState,
    pub log_selection: ListState,
    /// Active log filters (logs view).
    pub log_filters: LogFilters,
    /// Distinct units observed in the most recent logs fetch, sorted.
    pub known_units: Vec<String>,
    /// Set when logs must be re-fetched (view entered or filters changed).
    pub logs_stale: bool,
    /// Open drill-down pane, if any.
    pub detail: Option<DetailState>,
    /// Logs related to the drilled-down issue (fetched per unit).
    pub related_logs: Option<LogsResponse>,
    /// Set when the drill-down's related logs must be fetched.
    pub related_stale: bool,
    /// Active log search (logs view), if any.
    pub search: Option<SearchState>,
    /// When logs were last fetched (cache freshness).
    pub logs_fetched_at: Option<std::time::Instant>,
    /// Refresh cadence; how long cached logs stay fresh on view re-entry.
    pub refresh_interval: std::time::Duration,
}

impl App {
    #[must_use]
    pub fn new() -> Self {
        Self {
            issue_selection: ListState::default(),
            log_selection: ListState::default(),
            ..Self::default()
        }
    }

    /// Set the initial view (used for `default_view` in the config file).
    #[must_use]
    pub fn with_mode(mut self, mode: ViewMode) -> Self {
        self.mode = mode;
        self
    }

    /// Set the refresh cadence (used for cache freshness decisions).
    #[must_use]
    pub fn with_refresh_interval(mut self, interval: std::time::Duration) -> Self {
        self.refresh_interval = interval;
        self
    }

    /// Switch to another view, marking logs stale on entry.
    ///
    /// Re-entering the logs view reuses cached data while it is fresher than
    /// one refresh interval.
    pub fn set_mode(&mut self, mode: ViewMode) {
        if mode != self.mode {
            if mode == ViewMode::Logs {
                let fresh = self
                    .logs_fetched_at
                    .is_some_and(|t| t.elapsed() < self.refresh_interval);
                self.logs_stale = !fresh;
            }
            self.mode = mode;
        }
    }

    /// Handle a key press.
    pub fn on_key(&mut self, key: KeyEvent) {
        if self.show_help {
            if matches!(
                key.code,
                KeyCode::Char('?' | 'q' | 'Q') | KeyCode::Esc | KeyCode::Enter
            ) {
                self.show_help = false;
            }
            return;
        }

        if self.detail.is_some() {
            match key.code {
                KeyCode::Esc
                | KeyCode::Enter
                | KeyCode::Left
                | KeyCode::Char('?' | 'q' | 'Q' | 'b') => self.close_detail(),
                KeyCode::Down | KeyCode::Char('j') => {
                    if let Some(d) = &mut self.detail {
                        d.scroll = d.scroll.saturating_add(1);
                    }
                }
                KeyCode::Up | KeyCode::Char('k') => {
                    if let Some(d) = &mut self.detail {
                        d.scroll = d.scroll.saturating_sub(1);
                    }
                }
                _ => {}
            }
            return;
        }

        if self.search.is_some() {
            self.on_search_key(key.code);
            return;
        }

        if key.modifiers.contains(KeyModifiers::CONTROL)
            && matches!(key.code, KeyCode::Char('c' | 'C'))
        {
            self.should_quit = true;
            return;
        }

        match key.code {
            KeyCode::Char('q' | 'Q') | KeyCode::Esc => self.should_quit = true,
            KeyCode::Char('?') => self.show_help = true,
            KeyCode::Tab => self.set_mode(self.mode.next()),
            KeyCode::BackTab => self.set_mode(self.mode.previous()),
            KeyCode::Char('1') => self.set_mode(ViewMode::Summary),
            KeyCode::Char('2') => self.set_mode(ViewMode::Detailed),
            KeyCode::Char('3') => self.set_mode(ViewMode::Logs),
            KeyCode::Enter => self.open_detail(),
            KeyCode::Char('/') if self.mode == ViewMode::Logs => {
                self.search = Some(SearchState::default());
            }
            KeyCode::Down | KeyCode::Char('j') => self.scroll_active(true),
            KeyCode::Up | KeyCode::Char('k') => self.scroll_active(false),
            _ => {
                if self.mode == ViewMode::Logs {
                    self.on_logs_key(key.code);
                }
            }
        }
    }

    /// Open the drill-down pane for the current selection, if any.
    fn open_detail(&mut self) {
        let kind = match self.mode {
            ViewMode::Summary | ViewMode::Detailed => self
                .issue_selection
                .selected()
                .filter(|&i| self.health.as_ref().is_some_and(|h| i < h.issues.len()))
                .map(|index| {
                    let title = self
                        .health
                        .as_ref()
                        .map_or(String::new(), |h| h.issues[index].title.clone());
                    DrillDown::Issue {
                        index,
                        unit: unit_from_issue_title(&title),
                    }
                }),
            ViewMode::Logs => self
                .log_selection
                .selected()
                .filter(|&i| self.logs.as_ref().is_some_and(|l| i < l.entries.len()))
                .map(|index| DrillDown::Log { index }),
        };

        if let Some(kind) = kind {
            if matches!(kind, DrillDown::Issue { .. }) {
                self.related_stale = true;
            }
            self.detail = Some(DetailState { kind, scroll: 0 });
        }
    }

    /// Close the drill-down pane and discard related data.
    pub fn close_detail(&mut self) {
        self.detail = None;
        self.related_logs = None;
        self.related_stale = false;
    }

    /// Keys handled while the search prompt is open.
    fn on_search_key(&mut self, code: KeyCode) {
        let Some(search) = &mut self.search else {
            return;
        };

        // Navigation only applies once a pattern is committed.
        if search.is_committed() {
            match code {
                KeyCode::Char('n') => {
                    self.step_search(true);
                    return;
                }
                KeyCode::Char('N') => {
                    self.step_search(false);
                    return;
                }
                _ => {}
            }
        }

        match code {
            KeyCode::Esc => self.search = None,
            KeyCode::Enter => self.commit_search(),
            KeyCode::Backspace => {
                let Some(search) = &mut self.search else {
                    return;
                };
                search.input.pop();
                search.regex = None;
                search.matches.clear();
                search.error = None;
            }
            KeyCode::Char(c) => {
                let Some(search) = &mut self.search else {
                    return;
                };
                search.input.push(c);
                search.regex = None;
                search.matches.clear();
                search.error = None;
            }
            _ => {}
        }
    }

    /// Compile the current input and jump to the first match.
    fn commit_search(&mut self) {
        let pattern = self
            .search
            .as_ref()
            .map_or(String::new(), |s| s.input.clone());
        if pattern.is_empty() {
            self.search = None;
            return;
        }

        match regex::Regex::new(&pattern) {
            Ok(re) => {
                let matches: Vec<usize> = self.logs.as_ref().map_or_else(Vec::new, |l| {
                    l.entries
                        .iter()
                        .enumerate()
                        .filter(|(_, e)| re.is_match(&e.message))
                        .map(|(i, _)| i)
                        .collect()
                });
                if let Some(search) = &mut self.search {
                    search.regex = Some(re);
                    search.matches = matches;
                    search.current = 0;
                    search.error = None;
                }
                self.jump_to_current_match();
            }
            Err(err) => {
                if let Some(search) = &mut self.search {
                    search.error = Some(err.to_string());
                }
            }
        }
    }

    /// Move to the next/previous match and select it.
    fn step_search(&mut self, forward: bool) {
        let Some(search) = &mut self.search else {
            return;
        };
        let len = search.matches.len();
        if len == 0 {
            return;
        }
        search.current = if forward {
            (search.current + 1) % len
        } else {
            (search.current + len - 1) % len
        };
        self.jump_to_current_match();
    }

    /// Select the log entry for the current search match.
    fn jump_to_current_match(&mut self) {
        let target = self
            .search
            .as_ref()
            .and_then(|s| s.matches.get(s.current).copied());
        if let Some(index) = target {
            self.log_selection.select(Some(index));
        }
    }

    /// Filter shortcuts only active in the logs view.
    fn on_logs_key(&mut self, code: KeyCode) {
        match code {
            KeyCode::Char('a') => {
                self.log_filters.severity = match self.log_filters.severity {
                    None => Some(SeverityFilter::Error),
                    Some(SeverityFilter::Error) => Some(SeverityFilter::Warning),
                    Some(SeverityFilter::Warning) => Some(SeverityFilter::Info),
                    Some(SeverityFilter::Info) => None,
                };
                self.apply_filter_change();
            }
            KeyCode::Char('u') => {
                let units = &self.known_units;
                let next = match self.log_filters.unit.as_deref() {
                    None => units.first().cloned(),
                    Some(current) => units
                        .iter()
                        .position(|u| u == current)
                        .and_then(|i| units.get(i + 1))
                        .cloned(),
                };
                self.log_filters.unit = next;
                self.apply_filter_change();
            }
            KeyCode::Char('t') => {
                self.log_filters.time = self.log_filters.time.next();
                self.apply_filter_change();
            }
            KeyCode::Char('r') => {
                self.log_filters.reset();
                self.invalidate_logs();
            }
            KeyCode::PageDown | KeyCode::Char('.') => self.next_log_page(),
            KeyCode::PageUp | KeyCode::Char(',') => self.previous_log_page(),
            _ => {}
        }
    }

    /// Apply a filter change: back to page 0 and re-fetch.
    fn apply_filter_change(&mut self) {
        self.log_filters.offset = 0;
        self.invalidate_logs();
    }

    /// Advance to the next page of logs, if more data exists.
    fn next_log_page(&mut self) {
        let total = self.logs.as_ref().map_or(0, |l| l.total);
        if self.log_filters.offset + self.log_filters.page_size < total {
            self.log_filters.offset += self.log_filters.page_size;
            self.invalidate_logs();
        }
    }

    /// Go back one page of logs, if not already at the start.
    fn previous_log_page(&mut self) {
        if self.log_filters.offset > 0 {
            self.log_filters.offset = self
                .log_filters
                .offset
                .saturating_sub(self.log_filters.page_size);
            self.invalidate_logs();
        }
    }

    /// Mark logs as needing a re-fetch and reset the log selection.
    fn invalidate_logs(&mut self) {
        self.logs_stale = true;
        self.log_selection.select(None);
    }

    /// Rebuild `known_units` from the most recent logs response.
    pub fn refresh_units_from_logs(&mut self) {
        let mut units: Vec<String> = self.logs.as_ref().map_or_else(Vec::new, |l| {
            l.entries.iter().filter_map(|e| e.unit.clone()).collect()
        });
        units.sort_unstable();
        units.dedup();
        // Drop the current unit filter if it is no longer observed.
        if let Some(ref current) = self.log_filters.unit {
            if !units.contains(current) {
                self.log_filters.unit = None;
            }
        }
        self.known_units = units;
    }

    /// Scroll the selection of whichever list belongs to the active view.
    pub fn scroll_active(&mut self, down: bool) {
        match self.mode {
            ViewMode::Summary | ViewMode::Detailed => {
                let len = self.health.as_ref().map_or(0, |h| h.issues.len());
                let current = self.issue_selection.selected();
                self.issue_selection.select(scrolled(current, len, down));
            }
            ViewMode::Logs => {
                let len = self.logs.as_ref().map_or(0, |l| l.entries.len());
                let current = self.log_selection.selected();
                self.log_selection.select(scrolled(current, len, down));
            }
        }
    }

    /// Keep selections valid after a refresh shrinks or empties the lists.
    pub fn clamp_selections(&mut self) {
        let issue_len = self.health.as_ref().map_or(0, |h| h.issues.len());
        self.issue_selection
            .select(clamped(self.issue_selection.selected(), issue_len));

        let log_len = self.logs.as_ref().map_or(0, |l| l.entries.len());
        self.log_selection
            .select(clamped(self.log_selection.selected(), log_len));

        // Close the drill-down if its target no longer exists.
        let detail_valid = self.detail.as_ref().is_some_and(|d| match d.kind {
            DrillDown::Issue { index, .. } => index < issue_len,
            DrillDown::Log { index } => index < log_len,
        });
        if !detail_valid {
            self.detail = None;
            self.related_logs = None;
            self.related_stale = false;
        }
    }
}

/// Compute the next selection index after a scroll step.
fn scrolled(current: Option<usize>, len: usize, down: bool) -> Option<usize> {
    if len == 0 {
        return None;
    }
    let max = len - 1;
    let cur = current.unwrap_or(0);
    if down {
        Some(cur.saturating_add(1).min(max))
    } else {
        Some(cur.saturating_sub(1))
    }
}

/// Clamp an existing selection to the current list length.
fn clamped(current: Option<usize>, len: usize) -> Option<usize> {
    match (current, len) {
        (_, 0) => None,
        (None, _) => Some(0),
        (Some(i), n) => Some(i.min(n - 1)),
    }
}

/// Extract a systemd unit name from an issue title, if one is mentioned.
///
/// Titles such as `"Failed Unit: nginx.service"` yield `nginx.service`.
#[must_use]
fn unit_from_issue_title(title: &str) -> Option<String> {
    const SUFFIXES: [&str; 4] = [".service", ".timer", ".socket", ".mount"];
    title.split_whitespace().rev().find_map(|token| {
        let token = token.trim_matches(|c: char| !c.is_alphanumeric() && c != '.' && c != '-');
        SUFFIXES
            .iter()
            .any(|s| token.ends_with(s))
            .then(|| token.to_string())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    #[test]
    fn view_mode_cycles_forward_and_back() {
        assert_eq!(ViewMode::Summary.next(), ViewMode::Detailed);
        assert_eq!(ViewMode::Detailed.next(), ViewMode::Logs);
        assert_eq!(ViewMode::Logs.next(), ViewMode::Summary);
        assert_eq!(ViewMode::Summary.previous(), ViewMode::Logs);
        assert_eq!(ViewMode::Logs.previous(), ViewMode::Detailed);
    }

    #[test]
    fn tab_switches_modes() {
        let mut app = App::new();
        app.on_key(key(KeyCode::Tab));
        assert_eq!(app.mode, ViewMode::Detailed);
        app.on_key(key(KeyCode::BackTab));
        assert_eq!(app.mode, ViewMode::Summary);
    }

    #[test]
    fn number_keys_jump_to_mode() {
        let mut app = App::new();
        app.on_key(key(KeyCode::Char('3')));
        assert_eq!(app.mode, ViewMode::Logs);
        assert!(app.logs_stale);
        app.on_key(key(KeyCode::Char('1')));
        assert_eq!(app.mode, ViewMode::Summary);
    }

    #[test]
    fn quit_keys_work_and_help_intercepts_first() {
        let mut app = App::new();
        app.on_key(key(KeyCode::Char('?')));
        assert!(app.show_help);
        app.on_key(key(KeyCode::Char('q')));
        assert!(!app.should_quit, "help overlay should swallow keys");
        assert!(!app.show_help);

        app.on_key(key(KeyCode::Esc));
        assert!(app.should_quit);
    }

    #[test]
    fn ctrl_c_quits() {
        let mut app = App::new();
        app.on_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL));
        assert!(app.should_quit);
    }

    #[test]
    fn scroll_clamps_at_bounds() {
        let mut app = App::new();
        app.health = Some(HealthResponse {
            status: "good".into(),
            score: 80.0,
            raw_score: 80.0,
            heartbeat: "green".into(),
            timestamp: 0,
            breakdown: vitals_core::api::IssueBreakdown {
                errors: 0,
                warnings: 0,
                info: 0,
                total: 0,
            },
            issues: vec![
                vitals_core::api::IssueImpact {
                    id: "a".into(),
                    title: "A".into(),
                    severity: vitals_core::Severity::Info,
                    count: 1,
                    impact: -1.0,
                },
                vitals_core::api::IssueImpact {
                    id: "b".into(),
                    title: "B".into(),
                    severity: vitals_core::Severity::Warning,
                    count: 2,
                    impact: -2.0,
                },
            ],
            resources: None,
        });

        app.on_key(key(KeyCode::Down));
        app.on_key(key(KeyCode::Down));
        app.on_key(key(KeyCode::Down));
        assert_eq!(app.issue_selection.selected(), Some(1));

        app.on_key(key(KeyCode::Down));
        assert_eq!(app.issue_selection.selected(), Some(1), "clamped at end");

        app.on_key(key(KeyCode::Up));
        app.on_key(key(KeyCode::Up));
        app.on_key(key(KeyCode::Up));
        assert_eq!(app.issue_selection.selected(), Some(0), "clamped at start");
    }

    #[test]
    fn scroll_on_empty_list_is_noop() {
        let mut app = App::new();
        app.on_key(key(KeyCode::Down));
        assert_eq!(app.issue_selection.selected(), None);
    }

    #[test]
    fn clamp_selections_shrinks_with_data() {
        let mut app = App::new();
        app.log_selection.select(Some(5));
        app.clamp_selections();
        assert_eq!(app.log_selection.selected(), None, "empty logs -> none");

        app.logs = Some(LogsResponse {
            total: 2,
            entries: vec![log_entry("first"), log_entry("second")],
            time_window: vitals_core::api::TimeWindow {
                start: time::OffsetDateTime::UNIX_EPOCH,
                end: time::OffsetDateTime::UNIX_EPOCH,
                description: "test".into(),
            },
        });
        app.log_selection.select(Some(5));
        app.clamp_selections();
        assert_eq!(
            app.log_selection.selected(),
            Some(1),
            "clamped to last entry"
        );
    }

    #[test]
    fn severity_filter_cycles_and_invalidates() {
        let mut app = App::new();
        app.set_mode(ViewMode::Logs);
        app.on_key(key(KeyCode::Char('a')));
        assert_eq!(app.log_filters.severity, Some(SeverityFilter::Error));
        assert!(app.logs_stale);
        app.on_key(key(KeyCode::Char('a')));
        assert_eq!(app.log_filters.severity, Some(SeverityFilter::Warning));
        app.on_key(key(KeyCode::Char('a')));
        assert_eq!(app.log_filters.severity, Some(SeverityFilter::Info));
        app.on_key(key(KeyCode::Char('a')));
        assert_eq!(app.log_filters.severity, None);
    }

    #[test]
    fn unit_filter_cycles_through_known_units_then_wraps() {
        let mut app = App::new();
        app.set_mode(ViewMode::Logs);
        app.known_units = vec!["a.service".into(), "b.service".into()];

        app.on_key(key(KeyCode::Char('u')));
        assert_eq!(app.log_filters.unit.as_deref(), Some("a.service"));
        app.on_key(key(KeyCode::Char('u')));
        assert_eq!(app.log_filters.unit.as_deref(), Some("b.service"));
        app.on_key(key(KeyCode::Char('u')));
        assert_eq!(app.log_filters.unit, None);
    }

    #[test]
    fn time_preset_cycles_and_reset_clears_all() {
        let mut app = App::new();
        app.set_mode(ViewMode::Logs);
        app.on_key(key(KeyCode::Char('t')));
        assert_eq!(app.log_filters.time, TimePreset::Hour1);
        app.on_key(key(KeyCode::Char('t')));
        assert_eq!(app.log_filters.time, TimePreset::Hour6);

        app.on_key(key(KeyCode::Char('a')));
        app.on_key(key(KeyCode::Char('u')));
        app.on_key(key(KeyCode::Char('r')));
        assert_eq!(app.log_filters.severity, None);
        assert_eq!(app.log_filters.unit, None);
        assert_eq!(app.log_filters.time, TimePreset::Window);
    }

    #[test]
    fn filters_map_to_query_with_since_anchor() {
        let now = time::OffsetDateTime::UNIX_EPOCH;
        let mut filters = LogFilters {
            severity: Some(SeverityFilter::Warning),
            unit: Some("x.service".into()),
            time: TimePreset::Hour1,
            ..LogFilters::default()
        };
        let q = filters.to_query(now);
        assert_eq!(q.severity, Some(SeverityFilter::Warning));
        assert_eq!(q.unit.as_deref(), Some("x.service"));
        assert_eq!(q.since, Some(now - time::Duration::hours(1)));

        filters.time = TimePreset::Window;
        assert_eq!(filters.to_query(now).since, None);
    }

    #[test]
    fn refresh_units_derives_sorted_list_and_drops_stale_filter() {
        let mut app = App::new();
        app.logs = Some(LogsResponse {
            total: 2,
            entries: vec![log_entry_unit("z.service"), log_entry_unit("a.service")],
            time_window: vitals_core::api::TimeWindow {
                start: time::OffsetDateTime::UNIX_EPOCH,
                end: time::OffsetDateTime::UNIX_EPOCH,
                description: "test".into(),
            },
        });
        app.log_filters.unit = Some("gone.service".into());

        app.refresh_units_from_logs();

        assert_eq!(app.known_units, vec!["a.service", "z.service"]);
        assert_eq!(
            app.log_filters.unit, None,
            "filter for unobserved unit resets"
        );
    }

    #[test]
    fn enter_opens_issue_detail_and_esc_closes() {
        let mut app = App::new();
        app.health = Some(two_issue_health());
        app.issue_selection.select(Some(0));

        app.on_key(key(KeyCode::Enter));
        assert_eq!(
            app.detail.as_ref().map(|d| d.kind.clone()),
            Some(DrillDown::Issue {
                index: 0,
                unit: None
            })
        );
        assert!(app.related_stale);

        app.on_key(key(KeyCode::Esc));
        assert!(app.detail.is_none());
        assert!(!app.related_stale);
    }

    #[test]
    fn issue_detail_extracts_unit_from_title() {
        let mut app = App::new();
        let mut health = two_issue_health();
        health.issues[0].title = "Failed Unit: postgresql.service".into();
        app.health = Some(health);
        app.issue_selection.select(Some(0));

        app.on_key(key(KeyCode::Enter));
        assert_eq!(
            app.detail.as_ref().map(|d| d.kind.clone()),
            Some(DrillDown::Issue {
                index: 0,
                unit: Some("postgresql.service".to_string())
            })
        );
    }

    #[test]
    fn enter_opens_log_detail() {
        let mut app = App::new();
        app.set_mode(ViewMode::Logs);
        app.logs = Some(LogsResponse {
            total: 1,
            entries: vec![log_entry("hello")],
            time_window: test_window(),
        });
        app.log_selection.select(Some(0));

        app.on_key(key(KeyCode::Enter));
        assert_eq!(
            app.detail.as_ref().map(|d| d.kind.clone()),
            Some(DrillDown::Log { index: 0 })
        );

        // j scrolls, then Enter closes.
        app.on_key(key(KeyCode::Char('j')));
        assert_eq!(app.detail.as_ref().expect("open").scroll, 1);
        app.on_key(key(KeyCode::Enter));
        assert!(app.detail.is_none());
    }

    #[test]
    fn detail_closes_when_target_disappears_on_refresh() {
        let mut app = App::new();
        app.health = Some(two_issue_health());
        app.issue_selection.select(Some(1));
        app.on_key(key(KeyCode::Enter));
        assert!(app.detail.is_some());

        // Refresh shrinks the issue list to nothing.
        app.health = Some(two_issue_health());
        app.health.as_mut().expect("health").issues.clear();
        app.clamp_selections();
        assert!(app.detail.is_none(), "stale drill-down must close");
    }

    #[test]
    fn search_typing_commit_and_navigation() {
        let mut app = App::new();
        app.set_mode(ViewMode::Logs);
        app.logs = Some(LogsResponse {
            total: 3,
            entries: vec![
                log_entry("disk failure imminent"),
                log_entry("all systems nominal"),
                log_entry("disk retry succeeded"),
            ],
            time_window: test_window(),
        });

        app.on_key(key(KeyCode::Char('/')));
        assert!(app.search.is_some());

        for ch in "disk".chars() {
            app.on_key(key(KeyCode::Char(ch)));
        }
        assert_eq!(
            app.search.as_ref().expect("open").input,
            "disk",
            "typing accumulates"
        );

        app.on_key(key(KeyCode::Enter));
        let search = app.search.as_ref().expect("still open");
        assert!(search.is_committed());
        assert_eq!(search.matches, vec![0, 2]);
        assert_eq!(app.log_selection.selected(), Some(0), "jumped to first");

        app.on_key(key(KeyCode::Char('n')));
        assert_eq!(app.log_selection.selected(), Some(2));
        app.on_key(key(KeyCode::Char('n')));
        assert_eq!(app.log_selection.selected(), Some(0), "wraps forward");
        app.on_key(key(KeyCode::Char('N')));
        assert_eq!(app.log_selection.selected(), Some(2), "wraps backward");
    }

    #[test]
    fn search_invalid_regex_reports_error_and_esc_cancels() {
        let mut app = App::new();
        app.set_mode(ViewMode::Logs);
        app.logs = Some(LogsResponse {
            total: 1,
            entries: vec![log_entry("hello")],
            time_window: test_window(),
        });

        app.on_key(key(KeyCode::Char('/')));
        app.on_key(key(KeyCode::Char('[')));
        app.on_key(key(KeyCode::Enter));
        assert!(
            app.search.as_ref().expect("open").error.is_some(),
            "invalid pattern surfaces error"
        );
        assert!(!app.should_quit);

        app.on_key(key(KeyCode::Esc));
        assert!(app.search.is_none());
    }

    #[test]
    fn search_edit_after_commit_invalidates_matches() {
        let mut app = App::new();
        app.set_mode(ViewMode::Logs);
        app.logs = Some(LogsResponse {
            total: 1,
            entries: vec![log_entry("abc")],
            time_window: test_window(),
        });
        app.on_key(key(KeyCode::Char('/')));
        app.on_key(key(KeyCode::Char('a')));
        app.on_key(key(KeyCode::Enter));
        assert!(app.search.as_ref().expect("open").is_committed());

        app.on_key(key(KeyCode::Char('z')));
        let search = app.search.as_ref().expect("open");
        assert!(!search.is_committed());
        assert!(search.matches.is_empty());
    }

    #[test]
    fn pagination_moves_within_total_bounds() {
        let mut app = App::new();
        app.set_mode(ViewMode::Logs);
        app.logs = Some(LogsResponse {
            total: 450,
            entries: vec![],
            time_window: test_window(),
        });

        // page_size 200: 0 -> 200 -> 400, then clamped (400+200 >= 450).
        app.logs = Some(LogsResponse {
            total: 450,
            entries: vec![log_entry("x"); 200],
            time_window: test_window(),
        });
        app.on_key(key(KeyCode::Char('.')));
        assert_eq!(app.log_filters.offset, 200);
        app.on_key(key(KeyCode::Char('.')));
        assert_eq!(app.log_filters.offset, 400);
        app.on_key(key(KeyCode::Char('.')));
        assert_eq!(app.log_filters.offset, 400, "no page past the end");

        app.on_key(key(KeyCode::Char(',')));
        assert_eq!(app.log_filters.offset, 200);
        app.on_key(key(KeyCode::Char(',')));
        app.on_key(key(KeyCode::Char(',')));
        assert_eq!(app.log_filters.offset, 0, "clamped at start");
    }

    #[test]
    fn filter_change_resets_offset() {
        let mut app = App::new();
        app.set_mode(ViewMode::Logs);
        app.log_filters.offset = 400;

        app.on_key(key(KeyCode::Char('a')));
        assert_eq!(app.log_filters.offset, 0, "severity change resets page");
    }

    #[test]
    fn reentering_logs_view_reuses_fresh_cache() {
        let mut app = App::new().with_refresh_interval(std::time::Duration::from_secs(2));
        app.set_mode(ViewMode::Logs);
        assert!(app.logs_stale, "first entry has no cache");

        app.logs_stale = false;
        app.logs_fetched_at = Some(std::time::Instant::now());
        app.set_mode(ViewMode::Summary);
        app.set_mode(ViewMode::Logs);
        assert!(!app.logs_stale, "fresh cache suppresses refetch");

        app.logs_fetched_at =
            std::time::Instant::now().checked_sub(std::time::Duration::from_secs(5));
        app.set_mode(ViewMode::Summary);
        app.set_mode(ViewMode::Logs);
        assert!(app.logs_stale, "stale cache triggers refetch");
    }

    #[test]
    fn unit_extraction_from_titles() {
        assert_eq!(
            unit_from_issue_title("Failed Unit: nginx.service").as_deref(),
            Some("nginx.service")
        );
        assert_eq!(unit_from_issue_title("3 Error Journal Events"), None);
        assert_eq!(
            unit_from_issue_title("Correlated Issues: a.timer, b.socket").as_deref(),
            Some("b.socket")
        );
    }

    fn two_issue_health() -> HealthResponse {
        HealthResponse {
            status: "good".into(),
            score: 80.0,
            raw_score: 80.0,
            heartbeat: "green".into(),
            timestamp: 0,
            breakdown: vitals_core::api::IssueBreakdown {
                errors: 0,
                warnings: 0,
                info: 0,
                total: 0,
            },
            issues: vec![
                vitals_core::api::IssueImpact {
                    id: "a".into(),
                    title: "A".into(),
                    severity: vitals_core::Severity::Info,
                    count: 1,
                    impact: -1.0,
                },
                vitals_core::api::IssueImpact {
                    id: "b".into(),
                    title: "B".into(),
                    severity: vitals_core::Severity::Warning,
                    count: 2,
                    impact: -2.0,
                },
            ],
            resources: None,
        }
    }

    fn test_window() -> vitals_core::api::TimeWindow {
        vitals_core::api::TimeWindow {
            start: time::OffsetDateTime::UNIX_EPOCH,
            end: time::OffsetDateTime::UNIX_EPOCH,
            description: "test".into(),
        }
    }

    fn log_entry(message: &str) -> vitals_core::api::LogEntry {
        let mut entry = log_entry_unit("test.service");
        entry.message = message.to_string();
        entry
    }

    fn log_entry_unit(unit: &str) -> vitals_core::api::LogEntry {
        vitals_core::api::LogEntry {
            timestamp: time::OffsetDateTime::UNIX_EPOCH,
            message: "msg".to_string(),
            severity: vitals_core::Severity::Info,
            unit: Some(unit.to_string()),
            pid: Some(1),
            count: 1,
            first_seen: None,
            last_seen: None,
        }
    }
}
