use std::io;

use anyhow::{Context, Result};
use clap::Parser;
use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEvent},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
    Terminal,
};
use vitals_core::api::HealthResponse;

mod client;
mod ui;

use client::DaemonClient;

/// Command-line arguments
#[derive(Parser, Debug)]
#[command(name = "vitals-tui")]
#[command(about = "Terminal UI for vitals system monitoring")]
struct Args {
    /// Daemon URL
    #[arg(
        long,
        default_value = "http://localhost:8080",
        help = "URL of the vitals daemon"
    )]
    daemon_url: String,

    /// Refresh interval in seconds
    #[arg(long, default_value = "2", help = "Refresh interval in seconds")]
    refresh: u64,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();

    // Check if we're in a TTY
    if !atty::is(atty::Stream::Stdout) {
        anyhow::bail!("vitals-tui requires a TTY. Use vitals-daemon for non-TTY output.");
    }

    // Initialize daemon client
    let client = DaemonClient::new(&args.daemon_url)?;

    // Setup terminal
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    // Run the TUI
    let res = run_tui(&mut terminal, client, args.refresh).await;

    // Restore terminal
    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )?;
    terminal.show_cursor()?;

    if let Err(err) = res {
        eprintln!("Error: {err:?}");
    }

    Ok(())
}

async fn run_tui(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    client: DaemonClient,
    refresh_secs: u64,
) -> Result<()> {
    let mut last_update = std::time::Instant::now();
    let refresh_interval = std::time::Duration::from_secs(refresh_secs);

    let mut health_data: Option<HealthResponse> = None;

    loop {
        // Fetch data if needed
        if health_data.is_none() || last_update.elapsed() >= refresh_interval {
            health_data = Some(
                client
                    .get_health()
                    .await
                    .context("Failed to fetch health data")?,
            );
            last_update = std::time::Instant::now();
        }

        // Render UI
        terminal.draw(|f| {
            let chunks = Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Length(3), // Health score
                    Constraint::Min(5),    // Issues
                    Constraint::Length(3), // Metrics bar
                ])
                .split(f.area());

            if let Some(ref data) = health_data {
                // Health score section
                let health_score = format_health_score(data);
                let health_para = Paragraph::new(health_score)
                    .block(Block::default().borders(Borders::ALL).title("Health Score"));
                f.render_widget(health_para, chunks[0]);

                // Issues section
                let issues_text = format_issues(data);
                let issues_para = Paragraph::new(issues_text)
                    .block(Block::default().borders(Borders::ALL).title("Issues"));
                f.render_widget(issues_para, chunks[1]);

                // Metrics bar
                let metrics_text = format_metrics(data);
                let metrics_para =
                    Paragraph::new(metrics_text).block(Block::default().borders(Borders::NONE));
                f.render_widget(metrics_para, chunks[2]);
            } else {
                let loading = Paragraph::new("Loading...")
                    .block(Block::default().borders(Borders::ALL).title("Status"));
                f.render_widget(loading, chunks[0]);
            }
        })?;

        // Handle input with timeout
        if event::poll(std::time::Duration::from_millis(100))? {
            if let Event::Key(key) = event::read()? {
                if should_quit(key) {
                    return Ok(());
                }
            }
        }
    }
}

fn format_health_score(data: &HealthResponse) -> Line<'_> {
    let score = data.score;
    let color = match data.heartbeat.as_str() {
        "green" => Color::Green,
        "yellow" => Color::Yellow,
        "red" => Color::Red,
        _ => Color::White,
    };

    Line::from(vec![
        Span::raw("Health Score: "),
        Span::styled(
            format!("{score:.1}/100"),
            Style::default().fg(color).add_modifier(Modifier::BOLD),
        ),
        Span::raw(format!(" ({})", data.status)),
    ])
}

fn format_issues(data: &HealthResponse) -> Vec<Line<'_>> {
    let mut lines = vec![];

    if data.breakdown.total == 0 {
        lines.push(Line::from(Span::styled(
            "No issues detected",
            Style::default().fg(Color::Green),
        )));
        return lines;
    }

    // Show top issues
    for issue in data.issues.iter().take(10) {
        let severity_color = match issue.severity {
            vitals_core::Severity::Error => Color::Red,
            vitals_core::Severity::Warning => Color::Yellow,
            vitals_core::Severity::Info => Color::Cyan,
        };

        let severity_str = match issue.severity {
            vitals_core::Severity::Error => "[ERR]",
            vitals_core::Severity::Warning => "[WARN]",
            vitals_core::Severity::Info => "[INFO]",
        };

        lines.push(Line::from(vec![
            Span::styled(
                format!("{:>3}×", issue.count),
                Style::default().fg(Color::DarkGray),
            ),
            Span::raw(" "),
            Span::styled(severity_str, Style::default().fg(severity_color)),
            Span::raw(" "),
            Span::raw(&issue.title),
        ]));
    }

    if data.issues.len() > 10 {
        lines.push(Line::from(Span::styled(
            format!("... and {} more", data.issues.len() - 10),
            Style::default().fg(Color::DarkGray),
        )));
    }

    lines
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

fn should_quit(key: KeyEvent) -> bool {
    matches!(key.code, KeyCode::Char('q' | 'Q') | KeyCode::Esc)
}
