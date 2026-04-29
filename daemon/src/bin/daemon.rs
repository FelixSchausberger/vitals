//! Vitals daemon binary.
//!
//! Monitors system health through journald and systemd integration.
//! Provides HTTP API endpoints and one-shot CLI mode.

use std::{
    sync::{Arc, RwLock},
    time::Duration,
};

use anyhow::{Context, Result};
use axum::{extract::State, http::StatusCode, response::Json, routing::get, Router};
use clap::Parser;
use scorer::ResourceBaseline;
use serde_json::{json, Value};
use time::OffsetDateTime;
use tokio::time::interval;
use tower_http::{compression::CompressionLayer, cors::CorsLayer};
use vitals_daemon::{
    agg::aggregate_issues_with_config,
    config::{validate_config, Config},
    data::{
        mock::{MockJournal, MockMetrics, MockSystemd},
        traits::{
            JournalEntry, JournalReader, Metrics, MetricsReader, SystemdReader, UnitMetrics,
            UnitMetricsReader,
        },
    },
    health::{score_to_heartbeat_color, score_to_status, HealthBreakdown, TwhsCalculator},
    history::ScoreHistory,
    notifier::Notifier,
    probes::ProbeState,
};

/// Shared application state
#[derive(Clone)]
struct AppState {
    health_data: Arc<RwLock<Option<HealthBreakdown>>>,
    history: Arc<RwLock<ScoreHistory>>,
    config: Arc<Config>,
    notifier: Arc<RwLock<Notifier>>,
}

/// Output format for one-shot mode
#[derive(Debug, Clone, clap::ValueEnum)]
enum OutputFormat {
    /// Human-readable format
    Human,
    /// JSON output
    Json,
    /// Prometheus exposition format
    Prometheus,
}

/// Command-line arguments for the daemon
#[derive(Parser, Debug)]
#[command(
    name = "vitals-daemon",
    about = "Lightweight system health monitoring daemon",
    version = "0.1.0",
    long_about = "Monitors system health through journald and systemd integration.\n\
                  Calculates health scores based on errors, warnings, and resource utilization.\n\n\
                  Examples:\n  \
                  vitals-daemon                    # Start HTTP daemon\n  \
                  vitals-daemon --once             # One-shot health check\n  \
                  vitals-daemon --once --explain   # Show score breakdown"
)]
struct Args {
    /// Run once and exit (don't start HTTP server)
    #[arg(long, help = "Run health check once and exit")]
    once: bool,

    /// Output format for one-shot mode
    #[arg(
        long,
        value_enum,
        default_value = "human",
        help = "Output format (human, json, prometheus)"
    )]
    format: OutputFormat,

    /// Explain how the health score is calculated
    #[arg(
        long,
        help = "Show detailed breakdown of score calculation (requires --once)"
    )]
    explain: bool,

    /// Configuration file path
    #[arg(long, help = "Path to configuration file")]
    config: Option<String>,

    /// Enable debug logging
    #[arg(long, help = "Enable debug output")]
    debug: bool,

    /// Operating mode for data source
    #[arg(long, default_value = "live", help = "Data source mode (mock, live)")]
    mode: String,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();
    let _ = tracing_subscriber::fmt().with_target(true).try_init();

    // Validate arguments
    if args.explain && !args.once {
        anyhow::bail!("--explain requires --once flag");
    }

    // Initialize logging
    if args.debug {
        eprintln!("Vitals daemon starting in debug mode");
        eprintln!("Args: {args:#?}");
    }

    // Load configuration
    let config = if let Some(config_path) = &args.config {
        Config::load_from_path(std::path::Path::new(config_path))
            .with_context(|| format!("Failed to load config from {config_path}"))?
    } else {
        Config::load().context("Failed to load configuration")?
    };

    // Validate configuration
    validate_config(&config).context("Configuration validation failed")?;

    if args.debug {
        eprintln!("Configuration loaded: {config:#?}");
    }

    // One-shot mode: calculate health once and exit
    if args.once {
        return run_once(&config, &args).await;
    }

    // Daemon mode: start HTTP server
    run_daemon(config, args).await
}

/// Run health calculation once and output result
async fn run_once(config: &Config, args: &Args) -> Result<()> {
    use vitals_daemon::data::{
        journal_sd::SystemdJournalReader, metrics_procfs::UnitMetricsCollector,
        metrics_sysinfo::SysinfoMetricsReader, systemd_zbus::ZbusSystemdReader,
    };

    let data = match args.mode.as_str() {
        "mock" => fetch_mock_data(config).await?,
        "live" => {
            let journal_reader =
                SystemdJournalReader::new().context("Failed to initialize journal reader")?;
            let systemd_reader = ZbusSystemdReader::new()
                .await
                .context("Failed to initialize systemd reader")?;
            let metrics_reader = SysinfoMetricsReader::new();
            let unit_metrics_reader = UnitMetricsCollector::new();
            fetch_live_data_reuse(
                config,
                &journal_reader,
                &systemd_reader,
                &metrics_reader,
                &unit_metrics_reader,
            )
            .await?
        }
        _ => anyhow::bail!("Unknown mode: {}", args.mode),
    };

    let FetchedData {
        issues,
        system_metrics,
        unit_metrics,
        ..
    } = data;

    // Calculate health
    let mut calculator = TwhsCalculator::new(config.twhs.clone(), ResourceBaseline::new());
    let breakdown = calculator.compute(&issues, system_metrics.as_ref(), unit_metrics.as_deref());

    // Output based on format
    match args.format {
        OutputFormat::Json => output_json(&breakdown),
        OutputFormat::Prometheus => output_prometheus(&breakdown),
        OutputFormat::Human => {
            if args.explain {
                output_explain(&breakdown);
            } else {
                output_human(&breakdown);
            }
        }
    }

    Ok(())
}

/// Output human-readable health status
fn output_human(breakdown: &HealthBreakdown) {
    let status = score_to_status(breakdown.smoothed_score);
    let heartbeat = score_to_heartbeat_color(breakdown.smoothed_score);

    println!(
        "Health: {:.1}/100 ({}) [{}]",
        breakdown.smoothed_score, status, heartbeat
    );

    if breakdown.total_issues > 0 {
        println!(
            "Issues: {} error(s), {} warning(s), {} info",
            breakdown.error_count, breakdown.warning_count, breakdown.info_count
        );
    }

    if let Some(ref resources) = breakdown.resource_metrics {
        println!(
            "Resources: CPU {:.1}% ({}), Memory {:.1}% ({})",
            resources.cpu_usage,
            resources.cpu_status,
            resources.memory_usage,
            resources.memory_status
        );
    }
}

/// Output detailed explanation of health score
fn output_explain(breakdown: &HealthBreakdown) {
    let status = score_to_status(breakdown.smoothed_score);

    println!(
        "Health Score: {:.1}/100 ({})",
        breakdown.smoothed_score, status
    );
    println!("Raw Score: {:.1}/100", breakdown.overall_score);
    println!();
    println!("Score Breakdown:");
    println!("  Base: 100.0");

    // Show issue impacts
    if !breakdown.issue_impacts.is_empty() {
        println!();
        println!("  Issue Impacts:");
        for impact in &breakdown.issue_impacts {
            println!(
                "    [{:?}] {} ({} occurrence{}) → {:.1}",
                impact.severity,
                impact.title,
                impact.count,
                if impact.count == 1 { "" } else { "s" },
                impact.impact
            );
        }

        let total_issue_impact: f64 = breakdown.issue_impacts.iter().map(|i| i.impact).sum();
        println!("    Total issue impact: {total_issue_impact:.1}");
    }

    // Show resource impacts
    if let Some(ref resources) = breakdown.resource_metrics {
        println!();
        println!("  Resource Impacts:");
        println!(
            "    CPU: {:.1}% ({:?})",
            resources.cpu_usage, resources.cpu_status
        );
        println!(
            "    Memory: {:.1}% ({:?})",
            resources.memory_usage, resources.memory_status
        );
        println!(
            "    Disk: {:.1}% ({:?})",
            resources.disk_usage, resources.disk_status
        );
        println!(
            "    Load: {:.2} ({:?})",
            resources.load_average, resources.load_status
        );
        println!(
            "    Total resource impact: {:.1}",
            resources.resource_impact
        );

        if !resources.top_resource_consumers.is_empty() {
            println!();
            println!("  Top Resource Consumers:");
            for consumer in &resources.top_resource_consumers {
                println!(
                    "    {} (CPU: {:.1}%, Memory: {}MB, Impact: {:.1})",
                    consumer.unit_name,
                    consumer.cpu_usage,
                    consumer.memory_mb,
                    consumer.impact_score
                );
            }
        }
    }

    println!();
    println!("Summary:");
    println!("  {} error(s)", breakdown.error_count);
    println!("  {} warning(s)", breakdown.warning_count);
    println!("  {} info message(s)", breakdown.info_count);
}

/// Output JSON format
fn output_json(breakdown: &HealthBreakdown) {
    let json_output = json!({
        "status": score_to_status(breakdown.smoothed_score),
        "score": breakdown.smoothed_score,
        "raw_score": breakdown.overall_score,
        "heartbeat": score_to_heartbeat_color(breakdown.smoothed_score),
        "timestamp": breakdown.timestamp.unix_timestamp(),
        "breakdown": {
            "errors": breakdown.error_count,
            "warnings": breakdown.warning_count,
            "info": breakdown.info_count,
            "total": breakdown.total_issues
        },
        "issues": breakdown.issue_impacts.iter().map(|impact| json!({
            "id": impact.id,
            "title": impact.title,
            "severity": impact.severity,
            "count": impact.count,
            "impact": impact.impact
        })).collect::<Vec<_>>(),
        "resources": breakdown.resource_metrics.as_ref().map(|rm| json!({
            "cpu_usage": rm.cpu_usage,
            "cpu_status": rm.cpu_status,
            "memory_usage": rm.memory_usage,
            "memory_status": rm.memory_status,
            "disk_usage": rm.disk_usage,
            "disk_status": rm.disk_status,
            "load_average": rm.load_average,
            "load_status": rm.load_status,
            "resource_impact": rm.resource_impact,
            "top_consumers": rm.top_resource_consumers.iter().map(|c| json!({
                "unit": c.unit_name,
                "cpu_usage": c.cpu_usage,
                "memory_mb": c.memory_mb,
                "impact_score": c.impact_score
            })).collect::<Vec<_>>()
        }))
    });

    println!("{}", serde_json::to_string_pretty(&json_output).unwrap());
}

/// Output Prometheus exposition format
fn output_prometheus(breakdown: &HealthBreakdown) {
    let timestamp_ms = breakdown.timestamp.unix_timestamp_nanos() / 1_000_000;

    println!(
        r#"# HELP vitals_health_score Current system health score (0-100)
# TYPE vitals_health_score gauge
vitals_health_score{{type="raw"}} {raw_score} {timestamp}
vitals_health_score{{type="smoothed"}} {smoothed_score} {timestamp}

# HELP vitals_issues_total Total number of issues by severity
# TYPE vitals_issues_total counter
vitals_issues_total{{severity="error"}} {error_count} {timestamp}
vitals_issues_total{{severity="warning"}} {warning_count} {timestamp}
vitals_issues_total{{severity="info"}} {info_count} {timestamp}

# HELP vitals_health_status Current health status (0=critical, 1=poor, 2=fair, 3=good, 4=excellent)
# TYPE vitals_health_status gauge
vitals_health_status {status_value} {timestamp}"#,
        raw_score = breakdown.overall_score,
        smoothed_score = breakdown.smoothed_score,
        error_count = breakdown.error_count,
        warning_count = breakdown.warning_count,
        info_count = breakdown.info_count,
        status_value = status_to_metric_value(score_to_status(breakdown.smoothed_score)),
        timestamp = timestamp_ms
    );

    // Add resource metrics if available
    if let Some(ref rm) = breakdown.resource_metrics {
        println!(
            r#"
# HELP vitals_resource_usage Current resource utilization percentages
# TYPE vitals_resource_usage gauge
vitals_resource_usage{{type="cpu"}} {cpu_usage} {timestamp}
vitals_resource_usage{{type="memory"}} {memory_usage} {timestamp}
vitals_resource_usage{{type="disk"}} {disk_usage} {timestamp}

# HELP vitals_load_average Current system load average
# TYPE vitals_load_average gauge
vitals_load_average {load_average} {timestamp}"#,
            cpu_usage = rm.cpu_usage,
            memory_usage = rm.memory_usage,
            disk_usage = rm.disk_usage,
            load_average = rm.load_average,
            timestamp = timestamp_ms
        );
    }
}

/// Run daemon mode with HTTP server
async fn run_daemon(config: Config, args: Args) -> Result<()> {
    // Create shared state
    let state = AppState {
        health_data: Arc::new(RwLock::new(None)),
        history: Arc::new(RwLock::new(ScoreHistory::load_or_new())),
        config: Arc::new(config.clone()),
        notifier: Arc::new(RwLock::new(Notifier::new(config.notifier.clone()))),
    };

    // Start health calculation task
    let health_task_state = state.clone();
    let mode = args.mode.clone();
    let debug = args.debug;
    tokio::spawn(async move {
        if let Err(e) = run_health_calculator(health_task_state, mode, debug).await {
            eprintln!("Health calculation task failed: {e}");
        }
    });

    // Create HTTP server
    let app = create_app(state.clone());

    // Start server
    let addr = format!("{}:{}", config.daemon.host, config.daemon.port);
    println!("Vitals daemon listening on {addr}");
    println!("  Health endpoint: http://{addr}/health");
    println!("  Metrics endpoint: http://{addr}/metrics");

    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .with_context(|| format!("Failed to bind to {addr}"))?;

    axum::serve(listener, app)
        .await
        .context("HTTP server failed")?;

    Ok(())
}

/// Create the HTTP application router
fn create_app(state: AppState) -> Router {
    Router::new()
        .route("/health", get(health_handler))
        .route("/score", get(score_handler))
        .route("/metrics", get(metrics_handler))
        .route("/history", get(history_handler))
        .route("/", get(root_handler))
        .layer(CompressionLayer::new())
        .layer(CorsLayer::permissive())
        .with_state(state)
}

/// Root endpoint handler
async fn root_handler() -> Json<Value> {
    Json(json!({
        "name": "vitals-daemon",
        "version": "0.1.0",
        "endpoints": [
            "/health",
            "/score",
            "/metrics",
            "/history"
        ]
    }))
}

/// Score endpoint handler - returns current score with 1h delta
async fn score_handler(State(state): State<AppState>) -> Result<Json<Value>, StatusCode> {
    let health_data = state
        .health_data
        .read()
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let history = state
        .history
        .read()
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let (score, status) = match health_data.as_ref() {
        Some(breakdown) => (
            breakdown.smoothed_score,
            score_to_status(breakdown.smoothed_score),
        ),
        None => return Err(StatusCode::SERVICE_UNAVAILABLE),
    };

    let now_ts = OffsetDateTime::now_utc().unix_timestamp();
    let delta_1h = history.change_over_period(3600, score, now_ts);

    Ok(Json(json!({
        "score": score,
        "status": status,
        "delta_1h": delta_1h,
    })))
}

/// History endpoint handler - returns rolling 7-day score history
async fn history_handler(State(state): State<AppState>) -> Result<Json<Value>, StatusCode> {
    let history = state
        .history
        .read()
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let records: Vec<Value> = history
        .all_records()
        .iter()
        .map(|r| json!({"timestamp": r.timestamp, "score": r.score}))
        .collect();
    let now_ts = OffsetDateTime::now_utc().unix_timestamp();
    Ok(Json(json!({
        "records": records,
        "change_1h":  history.change_over_period(3600,      history.all_records().back().map_or(0.0, |r| r.score), now_ts),
        "change_24h": history.change_over_period(86400,     history.all_records().back().map_or(0.0, |r| r.score), now_ts),
        "change_7d":  history.change_over_period(7 * 86400, history.all_records().back().map_or(0.0, |r| r.score), now_ts),
    })))
}

/// Health endpoint handler - returns JSON health breakdown
async fn health_handler(State(state): State<AppState>) -> Result<Json<Value>, StatusCode> {
    let health_data = state
        .health_data
        .read()
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    health_data.as_ref().map_or_else(
        || {
            let response = json!({
                "status": "initializing",
                "message": "Health data not yet available"
            });
            Ok(Json(response))
        },
        |breakdown| {
            let mut response = json!({
                "status": score_to_status(breakdown.smoothed_score),
                "score": breakdown.smoothed_score,
                "raw_score": breakdown.overall_score,
                "heartbeat": score_to_heartbeat_color(breakdown.smoothed_score),
                "timestamp": breakdown.timestamp.unix_timestamp(),
                "breakdown": {
                    "errors": breakdown.error_count,
                    "warnings": breakdown.warning_count,
                    "info": breakdown.info_count,
                    "total": breakdown.total_issues
                },
                "issues": breakdown.issue_impacts.iter().map(|impact| json!({
                    "id": impact.id,
                    "title": impact.title,
                    "severity": impact.severity,
                    "count": impact.count,
                    "impact": impact.impact
                })).collect::<Vec<_>>()
            });

            // Add resource metrics if available
            if let Some(ref resource_metrics) = breakdown.resource_metrics {
                response["resources"] = json!({
                    "cpu_usage": resource_metrics.cpu_usage,
                    "cpu_status": resource_metrics.cpu_status,
                    "memory_usage": resource_metrics.memory_usage,
                    "memory_status": resource_metrics.memory_status,
                    "disk_usage": resource_metrics.disk_usage,
                    "disk_status": resource_metrics.disk_status,
                    "load_average": resource_metrics.load_average,
                    "load_status": resource_metrics.load_status,
                    "resource_impact": resource_metrics.resource_impact,
                    "resource_hog_count": resource_metrics.resource_hog_count,
                    "top_consumers": resource_metrics.top_resource_consumers.iter().map(|consumer| json!({
                        "unit": consumer.unit_name,
                        "cpu_usage": consumer.cpu_usage,
                        "memory_mb": consumer.memory_mb,
                        "impact_score": consumer.impact_score
                    })).collect::<Vec<_>>()
                });
            }

            let response = response;
            Ok(Json(response))
        },
    )
}

/// Metrics endpoint handler - returns Prometheus exposition format
async fn metrics_handler(State(state): State<AppState>) -> Result<String, StatusCode> {
    let health_data = state
        .health_data
        .read()
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    health_data.as_ref().map_or_else(
        || {
            let now = OffsetDateTime::now_utc();
            let timestamp_ms = now.unix_timestamp_nanos() / 1_000_000;

            let metrics = format!(
                r#"# HELP vitals_health_score Current system health score (0-100)
# TYPE vitals_health_score gauge
vitals_health_score{{type="raw"}} 0 {timestamp_ms}
vitals_health_score{{type="smoothed"}} 0 {timestamp_ms}

# HELP vitals_health_status Current health status (0=critical, 1=poor, 2=fair, 3=good, 4=excellent)
# TYPE vitals_health_status gauge
vitals_health_status -1 {timestamp_ms}
"#
            );
            Ok(metrics)
        },
        |breakdown| {
            let timestamp_ms = breakdown.timestamp.unix_timestamp_nanos() / 1_000_000;

            let mut metrics = format!(
                r#"# HELP vitals_health_score Current system health score (0-100)
# TYPE vitals_health_score gauge
vitals_health_score{{type="raw"}} {raw_score} {timestamp}
vitals_health_score{{type="smoothed"}} {smoothed_score} {timestamp}

# HELP vitals_issues_total Total number of issues by severity
# TYPE vitals_issues_total counter
vitals_issues_total{{severity="error"}} {error_count} {timestamp}
vitals_issues_total{{severity="warning"}} {warning_count} {timestamp}
vitals_issues_total{{severity="info"}} {info_count} {timestamp}

# HELP vitals_health_status Current health status (0=critical, 1=poor, 2=fair, 3=good, 4=excellent)
# TYPE vitals_health_status gauge
vitals_health_status {status_value} {timestamp}

# HELP vitals_last_update_timestamp Unix timestamp of last health calculation
# TYPE vitals_last_update_timestamp gauge
vitals_last_update_timestamp {timestamp_secs} {timestamp}
"#,
                raw_score = breakdown.overall_score,
                smoothed_score = breakdown.smoothed_score,
                error_count = breakdown.error_count,
                warning_count = breakdown.warning_count,
                info_count = breakdown.info_count,
                status_value = status_to_metric_value(score_to_status(breakdown.smoothed_score)),
                timestamp = timestamp_ms,
                timestamp_secs = breakdown.timestamp.unix_timestamp()
            );

            // Add resource metrics if available
            if let Some(ref resource_metrics) = breakdown.resource_metrics {
                let resource_metrics_str = format!(
                    r#"
# HELP vitals_resource_usage Current resource utilization percentages
# TYPE vitals_resource_usage gauge
vitals_resource_usage{{type="cpu"}} {cpu_usage} {timestamp}
vitals_resource_usage{{type="memory"}} {memory_usage} {timestamp}
vitals_resource_usage{{type="disk"}} {disk_usage} {timestamp}

# HELP vitals_load_average Current system load average
# TYPE vitals_load_average gauge
vitals_load_average {load_average} {timestamp}

# HELP vitals_resource_status Resource health status (0=healthy, 1=warning, 2=critical)
# TYPE vitals_resource_status gauge
vitals_resource_status{{type="cpu"}} {cpu_status} {timestamp}
vitals_resource_status{{type="memory"}} {memory_status} {timestamp}
vitals_resource_status{{type="disk"}} {disk_status} {timestamp}
vitals_resource_status{{type="load"}} {load_status} {timestamp}

# HELP vitals_resource_hogs_count Number of high-resource-consuming units
# TYPE vitals_resource_hogs_count gauge
vitals_resource_hogs_count {resource_hog_count} {timestamp}

# HELP vitals_resource_impact Resource utilization impact on health score
# TYPE vitals_resource_impact gauge
vitals_resource_impact {resource_impact} {timestamp}
"#,
                    cpu_usage = resource_metrics.cpu_usage,
                    memory_usage = resource_metrics.memory_usage,
                    disk_usage = resource_metrics.disk_usage,
                    load_average = resource_metrics.load_average,
                    cpu_status = resource_status_to_metric(&resource_metrics.cpu_status),
                    memory_status = resource_status_to_metric(&resource_metrics.memory_status),
                    disk_status = resource_status_to_metric(&resource_metrics.disk_status),
                    load_status = resource_status_to_metric(&resource_metrics.load_status),
                    resource_hog_count = resource_metrics.resource_hog_count,
                    resource_impact = resource_metrics.resource_impact,
                    timestamp = timestamp_ms
                );
                metrics.push_str(&resource_metrics_str);
            }

            Ok(metrics)
        },
    )
}

/// Convert status string to metric value
fn status_to_metric_value(status: &str) -> i32 {
    match status {
        "excellent" => 4,
        "good" => 3,
        "fair" => 2,
        "poor" => 1,
        "critical" => 0,
        _ => -1,
    }
}

/// Convert `ResourceStatus` to metric value
fn resource_status_to_metric(status: &vitals_daemon::health::ResourceStatus) -> i32 {
    match status {
        vitals_daemon::health::ResourceStatus::Healthy => 0,
        vitals_daemon::health::ResourceStatus::Warning => 1,
        vitals_daemon::health::ResourceStatus::Critical => 2,
    }
}

/// Run the health calculation loop.
///
/// Live-mode readers are constructed once before the loop so that:
/// - The journal reader's internal cursor accumulates across ticks (incremental reads)
/// - The sysinfo `System` instance is refreshed incrementally, not re-created
/// - The D-Bus connection is reused rather than re-established every tick
#[allow(clippy::too_many_lines)]
async fn run_health_calculator(state: AppState, mode: String, debug: bool) -> Result<()> {
    use vitals_daemon::data::{
        journal_sd::SystemdJournalReader, metrics_procfs::UnitMetricsCollector,
        metrics_sysinfo::SysinfoMetricsReader, systemd_zbus::ZbusSystemdReader,
    };

    let mut calculator = TwhsCalculator::new(state.config.twhs.clone(), ResourceBaseline::new());
    let mut probe_state = ProbeState::new();
    let mut tick_interval = interval(Duration::from_secs(
        state.config.daemon.calculation_interval,
    ));

    // Construct live readers once — reused across every tick
    let live_readers: Option<(
        SystemdJournalReader,
        ZbusSystemdReader,
        SysinfoMetricsReader,
        UnitMetricsCollector,
    )> = if mode == "live" {
        let journal = SystemdJournalReader::new().context("Failed to initialize journal reader")?;
        let systemd = ZbusSystemdReader::new()
            .await
            .context("Failed to initialize systemd reader")?;
        let metrics = SysinfoMetricsReader::new();
        let unit_metrics = UnitMetricsCollector::new();
        Some((journal, systemd, metrics, unit_metrics))
    } else {
        None
    };

    if debug {
        eprintln!("Health calculator started with {mode} mode");
    }

    loop {
        tick_interval.tick().await;

        if debug {
            eprintln!("Calculating health score...");
        }

        let result = match mode.as_str() {
            "mock" => fetch_mock_data(&state.config).await,
            "live" => {
                let (j, s, m, u) = live_readers.as_ref().expect("live readers must be Some");
                fetch_live_data_reuse(&state.config, j, s, m, u).await
            }
            _ => {
                eprintln!("Unknown mode: {mode}");
                continue;
            }
        };

        match result {
            Ok(fetched) => {
                // Collect active probe issues and merge with aggregated issues
                let probe_issues = probe_state.collect(
                    &fetched.journal_entries,
                    fetched.unit_metrics.as_deref(),
                    OffsetDateTime::now_utc(),
                );
                let agg_count = fetched.issues.len();
                let mut all_issues = fetched.issues;
                all_issues.extend(probe_issues);
                let probe_count = all_issues.len().saturating_sub(agg_count);

                let breakdown = calculator.compute(
                    &all_issues,
                    fetched.system_metrics.as_ref(),
                    fetched.unit_metrics.as_deref(),
                );

                if debug {
                    eprintln!(
                        "Health score: {:.1} (smoothed: {:.1}), {} issues ({} from probes)",
                        breakdown.overall_score,
                        breakdown.smoothed_score,
                        breakdown.total_issues,
                        probe_count,
                    );

                    if let Some(ref resource_metrics) = breakdown.resource_metrics {
                        eprintln!(
                            "Resource health: CPU {:.1}% ({:?}), Memory {:.1}% ({:?}), {} resource hogs",
                            resource_metrics.cpu_usage,
                            resource_metrics.cpu_status,
                            resource_metrics.memory_usage,
                            resource_metrics.memory_status,
                            resource_metrics.resource_hog_count
                        );
                    }
                }

                let score = breakdown.smoothed_score;
                if let Ok(mut health_data) = state.health_data.write() {
                    *health_data = Some(breakdown.clone());
                } else {
                    eprintln!("Failed to update health data");
                }

                if let Ok(mut history) = state.history.write() {
                    history.push(score, OffsetDateTime::now_utc());
                }

                let now_ts = OffsetDateTime::now_utc().unix_timestamp();
                let delta_1h = state
                    .history
                    .read()
                    .ok()
                    .and_then(|h| h.change_over_period(3600, score, now_ts));

                if let Ok(mut notifier) = state.notifier.write() {
                    notifier.notify(score, delta_1h, &breakdown);
                }
            }
            Err(e) => {
                eprintln!("Failed to fetch data for health calculation: {e}");
            }
        }
    }
}

/// Fetch data using mock sources.
async fn fetch_mock_data(config: &Config) -> Result<FetchedData> {
    let journal_reader = MockJournal;
    let systemd_reader = MockSystemd;
    let metrics_reader = MockMetrics;

    #[allow(clippy::cast_possible_wrap)]
    let since_time = OffsetDateTime::now_utc()
        - time::Duration::hours(config.daemon.journal_time_window_hours as i64);

    let (journal_entries, systemd_units, system_metrics) = tokio::try_join!(
        journal_reader.read_entries_since(Some(since_time), config.daemon.max_journal_entries),
        systemd_reader.list_units(),
        metrics_reader.get_metrics()
    )?;

    let issues =
        aggregate_issues_with_config(&journal_entries, &systemd_units, &config.aggregation);
    let unit_metrics = Some(create_mock_unit_metrics(&systemd_units));

    Ok(FetchedData {
        issues,
        system_metrics: Some(system_metrics),
        unit_metrics,
        journal_entries,
    })
}

/// Create mock unit metrics for testing resource monitoring
#[allow(clippy::cast_precision_loss)]
fn create_mock_unit_metrics(
    units: &[vitals_daemon::data::traits::SystemdUnit],
) -> Vec<vitals_daemon::data::traits::UnitMetrics> {
    units
        .iter()
        .enumerate()
        .map(|(i, unit)| vitals_daemon::data::traits::UnitMetrics {
            unit_name: unit.name.clone(),
            cpu_usage: (i as f64) * 15.0 % 100.0,
            memory_rss: ((i + 1) * 200_000_000) as u64,
            memory_vsz: ((i + 1) * 400_000_000) as u64,
            io_read_bps: (i as u64) * 1024 * 1024,
            io_write_bps: (i as u64) * 512 * 1024,
            net_rx_bps: (i as u64) * 100 * 1024,
            net_tx_bps: (i as u64) * 50 * 1024,
            pids: unit.pids.clone(),
        })
        .collect()
}

/// All data collected during a single health polling tick.
struct FetchedData {
    issues: Vec<vitals_daemon::model::Issue>,
    system_metrics: Option<Metrics>,
    unit_metrics: Option<Vec<UnitMetrics>>,
    /// Raw journal entries — passed to active probes for pattern scanning.
    journal_entries: Vec<JournalEntry>,
}

/// Fetch data using real system sources (readers constructed once and reused).
async fn fetch_live_data_reuse(
    config: &Config,
    journal_reader: &vitals_daemon::data::journal_sd::SystemdJournalReader,
    systemd_reader: &vitals_daemon::data::systemd_zbus::ZbusSystemdReader,
    metrics_reader: &vitals_daemon::data::metrics_sysinfo::SysinfoMetricsReader,
    unit_metrics_reader: &vitals_daemon::data::metrics_procfs::UnitMetricsCollector,
) -> Result<FetchedData> {
    #[allow(clippy::cast_possible_wrap)]
    let since_time = OffsetDateTime::now_utc()
        - time::Duration::hours(config.daemon.journal_time_window_hours as i64);

    let (journal_entries, systemd_units) = tokio::try_join!(
        journal_reader.read_entries_since(Some(since_time), config.daemon.max_journal_entries),
        systemd_reader.list_units()
    )?;

    let issues =
        aggregate_issues_with_config(&journal_entries, &systemd_units, &config.aggregation);

    let system_metrics = metrics_reader.get_metrics().await.ok();
    let unit_metrics = unit_metrics_reader.get_all_unit_metrics().await.ok();

    Ok(FetchedData {
        issues,
        system_metrics,
        unit_metrics,
        journal_entries,
    })
}
