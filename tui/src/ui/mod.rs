//! UI rendering for vitals TUI.
//!
//! All drawing lives here; state and input handling live in [`crate::app`].

use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, Paragraph, Wrap},
    Frame,
};
use vitals_core::{api::HealthResponse, Severity};

use crate::app::{App, ViewMode};

/// Render one frame.
pub fn draw(f: &mut Frame, app: &mut App) {
    let root = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1), // tab bar
            Constraint::Min(1),    // active view
            Constraint::Length(1), // hint bar
        ])
        .split(f.area());

    render_tabs(f, root[0], app.mode);

    match app.mode {
        ViewMode::Summary => draw_summary(f, root[1], app),
        ViewMode::Detailed => draw_detailed(f, root[1], app),
        ViewMode::Logs => draw_logs(f, root[1], app),
    }

    render_hints(f, root[2]);

    if app.show_help {
        render_help(f);
    } else if let Some(detail) = app.detail.as_ref() {
        render_detail(f, detail, app);
    }
}

fn render_tabs(f: &mut Frame, area: Rect, mode: ViewMode) {
    let mut spans = Vec::new();
    for (idx, view) in ViewMode::ALL.iter().enumerate() {
        let style = if *view == mode {
            Style::default()
                .fg(Color::Black)
                .bg(Color::Cyan)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::DarkGray)
        };
        spans.push(Span::raw(" "));
        spans.push(Span::styled(format!("{} {}", idx + 1, view.label()), style));
    }
    f.render_widget(Line::from(spans), area);
}

fn render_hints(f: &mut Frame, area: Rect) {
    let line = Line::from(vec![
        Span::styled(" Tab", Style::default().fg(Color::Cyan)),
        Span::raw(" views  "),
        Span::styled("\u{2191}\u{2193}", Style::default().fg(Color::Cyan)),
        Span::raw(" move  "),
        Span::styled("?", Style::default().fg(Color::Cyan)),
        Span::raw(" help  "),
        Span::styled("q", Style::default().fg(Color::Cyan)),
        Span::raw(" quit"),
    ]);
    f.render_widget(line.style(Style::default().fg(Color::DarkGray)), area);
}

fn draw_summary(f: &mut Frame, area: Rect, app: &mut App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3), // health score
            Constraint::Min(5),    // issues
            Constraint::Length(3), // metrics bar
        ])
        .split(area);

    let Some(data) = app.health.as_ref() else {
        render_loading(f, area);
        return;
    };

    let score = format_health_score(data);
    f.render_widget(
        Paragraph::new(score).block(Block::default().borders(Borders::ALL).title("Health Score")),
        chunks[0],
    );

    let items: Vec<ListItem> = data.issues.iter().map(issue_item).collect();
    let list = List::new(items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(format!("Issues ({})", data.issues.len())),
        )
        .highlight_style(Style::default().bg(Color::DarkGray).fg(Color::White));
    f.render_stateful_widget(list, chunks[1], &mut app.issue_selection);

    f.render_widget(
        Paragraph::new(format_metrics(data)).block(Block::default().borders(Borders::NONE)),
        chunks[2],
    );
}

fn draw_detailed(f: &mut Frame, area: Rect, app: &mut App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(5), // score detail
            Constraint::Length(6), // resources
            Constraint::Length(5), // top consumers
            Constraint::Min(5),    // all issues
        ])
        .split(area);

    let Some(data) = app.health.as_ref() else {
        render_loading(f, area);
        return;
    };

    let border = Block::default().borders(Borders::ALL);
    f.render_widget(
        Paragraph::new(score_detail_lines(data)).block(border.clone().title("Score Detail")),
        chunks[0],
    );

    let resource_lines = data.resources.as_ref().map_or_else(
        || vec![Line::from("Resource metrics not available")],
        resource_lines,
    );
    f.render_widget(
        Paragraph::new(resource_lines).block(border.clone().title("Resources")),
        chunks[1],
    );

    let consumers_text = data.resources.as_ref().map_or_else(
        || vec![Line::from("(no significant consumers)")],
        |r| {
            let lines = consumer_lines(r);
            if lines.is_empty() {
                vec![Line::from("(no significant consumers)")]
            } else {
                lines
            }
        },
    );
    f.render_widget(
        Paragraph::new(consumers_text).block(border.clone().title("Top Consumers")),
        chunks[2],
    );

    let items: Vec<ListItem> = data.issues.iter().map(detailed_issue_item).collect();
    let list = List::new(items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(format!("All Issues ({})", data.issues.len())),
        )
        .highlight_style(Style::default().bg(Color::DarkGray).fg(Color::White));
    f.render_stateful_widget(list, chunks[3], &mut app.issue_selection);
}

fn score_detail_lines(data: &HealthResponse) -> Vec<Line<'_>> {
    vec![
        format_health_score(data),
        Line::from(vec![
            Span::raw("Raw:      "),
            Span::styled(
                format!("{:.1}/100", data.raw_score),
                Style::default().fg(Color::Cyan),
            ),
            Span::raw("   Smoothed: "),
            Span::styled(
                format!("{:.1}/100", data.score),
                Style::default().fg(Color::Cyan),
            ),
        ]),
        Line::from(vec![
            Span::raw("Issues:   "),
            Span::styled(
                format!("{}", data.breakdown.errors),
                Style::default().fg(Color::Red),
            ),
            Span::raw(" errors, "),
            Span::styled(
                format!("{}", data.breakdown.warnings),
                Style::default().fg(Color::Yellow),
            ),
            Span::raw(" warnings, "),
            Span::styled(
                format!("{}", data.breakdown.info),
                Style::default().fg(Color::Cyan),
            ),
            Span::raw(format!(" info ({} total)", data.breakdown.total)),
        ]),
    ]
}

fn resource_lines(r: &vitals_core::api::ResourceMetrics) -> Vec<Line<'static>> {
    vec![
        metric_line("CPU  ", r.cpu_usage, &r.cpu_status),
        metric_line("MEM  ", r.memory_usage, &r.memory_status),
        metric_line("DISK ", r.disk_usage, &r.disk_status),
        Line::from(vec![
            Span::raw("LOAD  "),
            Span::styled(
                format!("{:.2}", r.load_average),
                Style::default().fg(Color::Cyan),
            ),
            Span::raw(format!("  ({})", r.load_status)),
            Span::raw(format!(
                "   impact {:.1}, {} hog(s)",
                r.resource_impact, r.resource_hog_count
            )),
        ]),
    ]
}

fn consumer_lines(r: &vitals_core::api::ResourceMetrics) -> Vec<Line<'static>> {
    r.top_consumers
        .iter()
        .take(3)
        .map(|c| {
            Line::from(vec![
                Span::raw(c.unit.clone()),
                Span::raw(format!(
                    "  cpu {:.1}%  mem {}MB  impact {:.1}",
                    c.cpu_usage, c.memory_mb, c.impact_score
                )),
            ])
        })
        .collect()
}

fn draw_logs(f: &mut Frame, area: Rect, app: &mut App) {
    let search_active = app.search.is_some();
    let mut constraints = vec![Constraint::Length(1), Constraint::Min(3)];
    if search_active {
        constraints.insert(1, Constraint::Length(1));
    }
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints(constraints)
        .split(area);

    render_log_filter_bar(f, chunks[0], app);
    let list_area = if search_active {
        if let Some(search) = app.search.as_ref() {
            render_search_prompt(f, chunks[1], search);
        }
        chunks[2]
    } else {
        chunks[1]
    };

    let Some(logs) = app.logs.as_ref() else {
        render_loading(f, list_area);
        return;
    };

    let highlight = app.search.as_ref().and_then(|s| s.regex.as_ref());
    let items: Vec<ListItem> = logs
        .entries
        .iter()
        .map(|e| ListItem::new(log_line(e, highlight)))
        .collect();
    let filters = &app.log_filters;
    let title = if logs.entries.len() == logs.total {
        format!(
            "Logs \u{2014} {} ({})",
            logs.time_window.description, logs.total
        )
    } else {
        format!(
            "Logs \u{2014} {} ({}-{} of {})",
            logs.time_window.description,
            filters.offset + 1,
            filters.offset + logs.entries.len(),
            logs.total
        )
    };
    let list = List::new(items)
        .block(Block::default().borders(Borders::ALL).title(title))
        .highlight_style(Style::default().bg(Color::DarkGray).fg(Color::White));
    f.render_stateful_widget(list, list_area, &mut app.log_selection);
}

fn render_search_prompt(f: &mut Frame, area: Rect, search: &crate::app::SearchState) {
    let mut spans = vec![
        Span::styled("/", Style::default().fg(Color::Cyan)),
        Span::raw(search.input.clone()),
    ];
    if let Some(ref err) = search.error {
        spans.push(Span::styled(
            format!("  {err}"),
            Style::default().fg(Color::Red),
        ));
    } else if search.is_committed() {
        spans.push(Span::styled(
            format!("  {}/{}", search.current + 1, search.matches.len()),
            Style::default().fg(Color::Cyan),
        ));
        spans.push(Span::styled("  n/N", Style::default().fg(Color::DarkGray)));
    } else {
        spans.push(Span::styled(
            "  (Enter to apply, Esc cancels)",
            Style::default().fg(Color::DarkGray),
        ));
    }
    f.render_widget(Line::from(spans), area);
}

fn render_log_filter_bar(f: &mut Frame, area: Rect, app: &App) {
    let filters = &app.log_filters;
    let severity_label = filters.severity.map_or("all", |s| match s {
        vitals_core::api::SeverityFilter::Error => "errors",
        vitals_core::api::SeverityFilter::Warning => "warnings",
        vitals_core::api::SeverityFilter::Info => "info",
    });
    let unit_label = filters.unit.as_deref().unwrap_or("all");
    let key_style = Style::default().fg(Color::Cyan);
    let value_style = Style::default().fg(Color::Yellow);
    let line = Line::from(vec![
        Span::styled(" a:", key_style),
        Span::styled(severity_label, value_style),
        Span::raw("  "),
        Span::styled("u:", key_style),
        Span::styled(unit_label.to_string(), value_style),
        Span::raw("  "),
        Span::styled("t:", key_style),
        Span::styled(filters.time.label(), value_style),
        Span::raw("  "),
        Span::styled("r", key_style),
        Span::raw(" reset"),
    ]);
    f.render_widget(line.style(Style::default().fg(Color::DarkGray)), area);
}

fn render_loading(f: &mut Frame, area: Rect) {
    f.render_widget(
        Paragraph::new("Connecting to vitals daemon\u{2026}")
            .block(Block::default().borders(Borders::ALL).title("Status")),
        area,
    );
}

fn render_help(f: &mut Frame) {
    let area = centered_rect(44, 16, f.area());
    f.render_widget(Clear, area);

    let lines = vec![
        Line::from(Span::styled(
            " Vitals TUI \u{2014} Keybindings",
            Style::default().add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        bind_line("q, Esc", "Quit"),
        bind_line("Ctrl+C", "Quit"),
        bind_line("Tab / BTab", "Cycle views"),
        bind_line("1 / 2 / 3", "Summary / Detailed / Logs"),
        bind_line("\u{2191} / k", "Move selection up"),
        bind_line("\u{2193} / j", "Move selection down"),
        bind_line("Enter", "Expand selection"),
        bind_line("a / u / t", "Logs: severity/unit/time filter"),
        bind_line("r", "Logs: reset filters"),
        bind_line(", / .", "Logs: prev/next page"),
        bind_line("/", "Logs: regex search"),
        bind_line("?", "Toggle this help"),
    ];

    f.render_widget(
        Paragraph::new(lines).block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Cyan)),
        ),
        area,
    );
}

fn bind_line(key: &str, action: &str) -> Line<'static> {
    Line::from(vec![
        Span::styled(format!(" {key:<10}"), Style::default().fg(Color::Cyan)),
        Span::raw(action.to_string()),
    ])
}

fn render_detail(f: &mut Frame, detail: &crate::app::DetailState, app: &App) {
    let area = f.area();
    let width = area.width * 70 / 100;
    let height = area.height * 60 / 100;
    let popup = centered_rect(width.max(30), height.max(8), area);
    f.render_widget(Clear, popup);

    match &detail.kind {
        crate::app::DrillDown::Issue { index, unit } => {
            render_issue_detail(f, popup, detail, app, *index, unit.as_deref());
        }
        crate::app::DrillDown::Log { index } => {
            render_log_detail(f, popup, detail, app, *index);
        }
    }
}

fn render_issue_detail(
    f: &mut Frame,
    area: Rect,
    detail: &crate::app::DetailState,
    app: &App,
    index: usize,
    unit: Option<&str>,
) {
    let Some(issue) = app.health.as_ref().and_then(|h| h.issues.get(index)) else {
        return;
    };
    let (tag, color) = severity_tag(issue.severity);

    let mut lines = vec![
        Line::from(vec![
            Span::styled(tag, Style::default().fg(color)),
            Span::raw(format!("  {}", issue.title)),
        ]),
        Line::from(""),
        Line::from(vec![
            Span::raw("Occurrences: "),
            Span::styled(issue.count.to_string(), Style::default().fg(Color::Cyan)),
        ]),
        Line::from(vec![
            Span::raw("Score impact: "),
            Span::styled(
                format!("{:.2}", issue.impact),
                Style::default().fg(Color::Red),
            ),
        ]),
        Line::from(vec![Span::raw("ID: "), Span::raw(issue.id.clone())]),
    ];

    if let Some(u) = unit {
        lines.push(Line::from(""));
        lines.push(Line::from(vec![
            Span::raw("Related logs for "),
            Span::styled(u.to_string(), Style::default().fg(Color::Blue)),
            Span::raw(":"),
        ]));
        match app.related_logs.as_ref() {
            Some(related) if related.entries.is_empty() => {
                lines.push(Line::from("  (no entries)"));
            }
            Some(related) => {
                for entry in related.entries.iter().take(20) {
                    lines.push(log_line(entry, None));
                }
            }
            None => lines.push(Line::from("  loading\u{2026}")),
        }
    }

    f.render_widget(
        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .scroll((detail.scroll, 0))
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(Color::Cyan))
                    .title(" Issue \u{2014} Esc to close "),
            ),
        area,
    );
}

fn render_log_detail(
    f: &mut Frame,
    area: Rect,
    detail: &crate::app::DetailState,
    app: &App,
    index: usize,
) {
    let Some(entry) = app.logs.as_ref().and_then(|l| l.entries.get(index)) else {
        return;
    };
    let (tag, color) = severity_tag(entry.severity);

    let mut lines = vec![
        Line::from(vec![
            Span::styled(tag, Style::default().fg(color)),
            Span::raw("  "),
            Span::styled(
                format_rfc3339_display(entry.timestamp),
                Style::default().fg(Color::DarkGray),
            ),
        ]),
        Line::from(""),
    ];
    if let Some(ref unit) = entry.unit {
        lines.push(Line::from(vec![
            Span::raw("Unit: "),
            Span::styled(unit.clone(), Style::default().fg(Color::Blue)),
        ]));
    }
    if let Some(pid) = entry.pid {
        lines.push(Line::from(format!("PID: {pid}")));
    }
    if entry.count > 1 {
        lines.push(Line::from(format!("Occurrences: {}", entry.count)));
    }
    if let Some(first) = entry.first_seen {
        lines.push(Line::from(format!(
            "First seen: {}",
            format_rfc3339_display(first)
        )));
    }
    if let Some(last) = entry.last_seen {
        lines.push(Line::from(format!(
            "Last seen: {}",
            format_rfc3339_display(last)
        )));
    }
    lines.push(Line::from(""));
    lines.push(Line::from(entry.message.clone()));

    f.render_widget(
        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .scroll((detail.scroll, 0))
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(Color::Cyan))
                    .title(" Log Entry \u{2014} Esc to close "),
            ),
        area,
    );
}

fn format_rfc3339_display(ts: time::OffsetDateTime) -> String {
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02} UTC",
        ts.year(),
        u8::from(ts.month()),
        ts.day(),
        ts.hour(),
        ts.minute(),
        ts.second()
    )
}

fn centered_rect(width: u16, height: u16, area: Rect) -> Rect {
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Fill(1),
            Constraint::Length(height),
            Constraint::Fill(1),
        ])
        .split(area);
    let horizontal = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Fill(1),
            Constraint::Length(width),
            Constraint::Fill(1),
        ])
        .split(vertical[1]);
    horizontal[1]
}

fn format_health_score(data: &HealthResponse) -> Line<'_> {
    let color = match data.heartbeat.as_str() {
        "green" => Color::Green,
        "yellow" => Color::Yellow,
        "red" => Color::Red,
        _ => Color::White,
    };

    Line::from(vec![
        Span::raw("Health Score: "),
        Span::styled(
            format!("{:.1}/100", data.score),
            Style::default().fg(color).add_modifier(Modifier::BOLD),
        ),
        Span::raw(format!(" ({})", data.status)),
    ])
}

fn format_metrics(data: &HealthResponse) -> Line<'_> {
    if let Some(ref resources) = data.resources {
        Line::from(vec![
            Span::raw("CPU "),
            Span::styled(
                format!("{:.1}%", resources.cpu_usage),
                Style::default().fg(Color::Cyan),
            ),
            Span::raw("  |  MEM "),
            Span::styled(
                format!("{:.1}%", resources.memory_usage),
                Style::default().fg(Color::Cyan),
            ),
            Span::raw("  |  DISK "),
            Span::styled(
                format!("{:.1}%", resources.disk_usage),
                Style::default().fg(Color::Cyan),
            ),
            Span::raw("  |  LOAD "),
            Span::styled(
                format!("{:.2}", resources.load_average),
                Style::default().fg(Color::Cyan),
            ),
        ])
    } else {
        Line::from("Resource metrics not available")
    }
}

fn metric_line(label: &str, usage: f64, status: &str) -> Line<'static> {
    Line::from(vec![
        Span::raw(label.to_string()),
        Span::styled(format!("{usage:.1}%"), Style::default().fg(Color::Cyan)),
        Span::raw(format!("  ({status})")),
    ])
}

fn severity_tag(severity: Severity) -> (&'static str, Color) {
    match severity {
        Severity::Error => ("[ERR]", Color::Red),
        Severity::Warning => ("[WARN]", Color::Yellow),
        Severity::Info => ("[INFO]", Color::Cyan),
    }
}

fn issue_item(issue: &vitals_core::api::IssueImpact) -> ListItem<'static> {
    let (tag, color) = severity_tag(issue.severity);
    ListItem::new(Line::from(vec![
        Span::styled(
            format!("{:>3}\u{d7}", issue.count),
            Style::default().fg(Color::DarkGray),
        ),
        Span::raw(" "),
        Span::styled(tag, Style::default().fg(color)),
        Span::raw(" "),
        Span::raw(issue.title.clone()),
    ]))
}

fn detailed_issue_item(issue: &vitals_core::api::IssueImpact) -> ListItem<'static> {
    let (tag, color) = severity_tag(issue.severity);
    ListItem::new(Line::from(vec![
        Span::styled(
            format!("{:>3}\u{d7}", issue.count),
            Style::default().fg(Color::DarkGray),
        ),
        Span::raw(" "),
        Span::styled(tag, Style::default().fg(color)),
        Span::raw(format!(" {:>6.1}  ", issue.impact)),
        Span::raw(issue.title.clone()),
    ]))
}

fn log_line(entry: &vitals_core::api::LogEntry, highlight: Option<&regex::Regex>) -> Line<'static> {
    let (tag, color) = severity_tag(entry.severity);
    let message_style = if highlight.is_some_and(|re| re.is_match(&entry.message)) {
        Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default()
    };
    let mut spans = vec![
        Span::styled(
            format_time(entry.timestamp),
            Style::default().fg(Color::DarkGray),
        ),
        Span::raw(" "),
        Span::styled(tag, Style::default().fg(color)),
        Span::raw(" "),
    ];
    if let Some(ref unit) = entry.unit {
        spans.push(Span::styled(unit.clone(), Style::default().fg(Color::Blue)));
        spans.push(Span::raw(" "));
    }
    if entry.count > 1 {
        spans.push(Span::styled(
            format!("\u{d7}{} ", entry.count),
            Style::default().fg(Color::DarkGray),
        ));
    }
    spans.push(Span::styled(entry.message.clone(), message_style));
    Line::from(spans)
}

fn format_time(ts: time::OffsetDateTime) -> String {
    format!("{:02}:{:02}:{:02}", ts.hour(), ts.minute(), ts.second())
}
