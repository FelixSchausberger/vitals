use std::{
    io::{self, IsTerminal},
    path::PathBuf,
    time::{Duration, Instant},
};

use anyhow::{Context, Result};
use clap::Parser;
use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{backend::CrosstermBackend, Terminal};

mod app;
mod client;
mod config;
mod ui;

use app::{App, ViewMode};
use client::DaemonClient;
use config::TuiConfig;
use time::OffsetDateTime;
use vitals_core::{
    addr::{resolve_daemon_addr, DaemonAddr},
    api::LogsQuery,
};

/// Command-line arguments
#[derive(Parser, Debug)]
#[command(name = "vitals-tui")]
#[command(about = "Terminal UI for vitals system monitoring")]
struct Args {
    /// Daemon URL (TCP)
    #[arg(long, help = "URL of the vitals daemon (e.g. http://localhost:8080)")]
    daemon_url: Option<String>,

    /// Daemon Unix socket path
    #[arg(long, help = "Unix socket path of the vitals daemon")]
    daemon_socket: Option<String>,

    /// Refresh interval in seconds
    #[arg(long, help = "Refresh interval in seconds [default: 2]")]
    refresh: Option<u64>,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();

    // Persistent settings; CLI arguments take precedence.
    let config = TuiConfig::load().context("Failed to load TUI configuration")?;

    // Check if we're in a TTY
    if !io::stdout().is_terminal() {
        anyhow::bail!("vitals-tui requires a TTY. Use vitals-daemon for non-TTY output.");
    }

    // Resolve daemon address
    let addr = resolve_daemon_addr_from_args(&args, &config);

    // Initialize daemon client
    let client = DaemonClient::from_addr(addr);

    // Setup terminal
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    // Run the TUI
    let refresh_secs = args.refresh.or(config.refresh_secs).unwrap_or(2);
    if let Some(raw) = config
        .default_view
        .as_deref()
        .filter(|v| view_from_str(v).is_none())
    {
        eprintln!("Ignoring unknown default_view {raw:?} in tui.toml");
    }
    let initial_view = config.default_view.as_deref().and_then(view_from_str);
    let app = App::new()
        .with_mode(initial_view.unwrap_or_default())
        .with_refresh_interval(Duration::from_secs(refresh_secs));
    let res = run_app(&mut terminal, client, app, refresh_secs).await;

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

/// Map a `default_view` config value to a view mode.
fn view_from_str(value: &str) -> Option<ViewMode> {
    match value.to_ascii_lowercase().as_str() {
        "summary" => Some(ViewMode::Summary),
        "detailed" => Some(ViewMode::Detailed),
        "logs" => Some(ViewMode::Logs),
        _ => None,
    }
}

fn resolve_daemon_addr_from_args(args: &Args, config: &TuiConfig) -> DaemonAddr {
    if let Some(ref path) = args.daemon_socket {
        return DaemonAddr::Unix {
            path: PathBuf::from(path),
        };
    }

    if let Some(ref url) = args.daemon_url {
        return DaemonAddr::Tcp {
            url: url.trim_end_matches('/').to_string(),
        };
    }

    if let Some(ref path) = config.daemon_socket {
        return DaemonAddr::Unix {
            path: PathBuf::from(path),
        };
    }

    if let Some(ref url) = config.daemon_url {
        return DaemonAddr::Tcp {
            url: url.trim_end_matches('/').to_string(),
        };
    }

    resolve_daemon_addr()
}

async fn run_app(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    client: DaemonClient,
    mut app: App,
    refresh_secs: u64,
) -> Result<()> {
    let refresh_interval = Duration::from_secs(refresh_secs);

    // Force an initial fetch on the first loop iteration.
    let mut last_fetch = Instant::now()
        .checked_sub(refresh_interval)
        .unwrap_or_else(Instant::now);

    loop {
        if last_fetch.elapsed() >= refresh_interval {
            app.health = Some(
                client
                    .get_health()
                    .await
                    .context("Failed to fetch health data")?,
            );
            if app.mode == ViewMode::Logs {
                let query = app.log_filters.to_query(OffsetDateTime::now_utc());
                app.logs = Some(
                    client
                        .get_logs(&query)
                        .await
                        .context("Failed to fetch logs")?,
                );
                app.logs_fetched_at = Some(Instant::now());
            }
            last_fetch = Instant::now();
            app.clamp_selections();
        }

        if app.logs_stale {
            let query = app.log_filters.to_query(OffsetDateTime::now_utc());
            app.logs = Some(
                client
                    .get_logs(&query)
                    .await
                    .context("Failed to fetch logs")?,
            );
            app.logs_stale = false;
            app.logs_fetched_at = Some(Instant::now());
            app.refresh_units_from_logs();
            app.clamp_selections();
        }

        if app.related_stale {
            let unit = app.detail.as_ref().and_then(|d| match &d.kind {
                app::DrillDown::Issue { unit, .. } => unit.clone(),
                app::DrillDown::Log { .. } => None,
            });
            let query = LogsQuery {
                unit,
                ..LogsQuery::default()
            };
            app.related_logs = Some(
                client
                    .get_logs(&query)
                    .await
                    .context("Failed to fetch related logs")?,
            );
            app.related_stale = false;
        }

        terminal.draw(|f| ui::draw(f, &mut app))?;

        if event::poll(Duration::from_millis(100))? {
            if let Event::Key(key) = event::read()? {
                app.on_key(key);
            }
        }

        if app.should_quit {
            return Ok(());
        }
    }
}
