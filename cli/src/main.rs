use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand, ValueEnum};
use serde::Deserialize;
use serde_json::Value;
use vitals_core::addr::{resolve_daemon_addr, DaemonAddr};

// ── CLI args ──────────────────────────────────────────────────────────────────

#[derive(Parser, Debug)]
#[command(
    name = "vitals",
    about = "Query the vitals health daemon",
    version = "0.1.0"
)]
struct Args {
    /// Daemon base URL (TCP)
    #[arg(long, env = "VITALS_URL")]
    url: Option<String>,

    /// Daemon Unix socket path
    #[arg(long, env = "VITALS_SOCKET")]
    socket: Option<String>,

    #[command(subcommand)]
    command: Cmd,
}

#[derive(Subcommand, Debug)]
enum Cmd {
    /// Print current health status
    Status {
        #[arg(long, value_enum, default_value = "human")]
        format: Format,
    },
    /// List active issues that impact the score
    Issues,
    /// Show the rolling 7-day score history
    History,
}

#[derive(ValueEnum, Debug, Clone)]
enum Format {
    Human,
    Ironbar,
    Json,
}

// ── HTTP response types ───────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct HealthResponse {
    score: f64,
    status: String,
    #[serde(default)]
    breakdown: Breakdown,
    #[serde(default)]
    issues: Vec<IssueImpact>,
    resources: Option<Resources>,
}

#[derive(Debug, Deserialize, Default)]
struct Breakdown {
    #[serde(default)]
    errors: usize,
    #[serde(default)]
    warnings: usize,
    #[serde(default)]
    info: usize,
}

#[allow(dead_code)]
#[derive(Debug, Deserialize)]
struct IssueImpact {
    title: String,
    severity: String,
    count: usize,
    impact: f64,
    #[serde(default)]
    unit: Option<String>,
    #[serde(default)]
    hints: Vec<String>,
}

#[allow(dead_code)]
#[derive(Debug, Deserialize)]
struct Resources {
    cpu_usage: f64,
    cpu_penalty: f64,
    memory_usage: f64,
    memory_penalty: f64,
    disk_usage: f64,
    disk_penalty: f64,
    load_average: f64,
    load_penalty: f64,
    #[serde(default)]
    resource_impact: f64,
    #[serde(default)]
    baseline_learning_mode: bool,
    #[serde(default)]
    baseline_samples: u64,
    #[serde(default)]
    min_samples_for_baseline: u64,
}

#[derive(Debug, Deserialize)]
struct HistoryResponse {
    records: Vec<HistoryRecord>,
    change_1h: Option<f64>,
    change_24h: Option<f64>,
    change_7d: Option<f64>,
}

#[derive(Debug, Deserialize)]
struct HistoryRecord {
    #[allow(dead_code)]
    timestamp: i64,
    score: f64,
}

// ── entry point ───────────────────────────────────────────────────────────────

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();

    let daemon_addr = resolve_daemon_addr_from_args(&args);

    match args.command {
        Cmd::Status { format } => {
            let health = fetch_health(&daemon_addr).await?;
            let history = fetch_history(&daemon_addr).await.ok();
            match format {
                Format::Human => print_human(&health, history.as_ref()),
                Format::Ironbar => print_ironbar(&health, history.as_ref()),
                Format::Json => {
                    let raw = fetch_json_value(&daemon_addr, "/health").await?;
                    println!("{}", serde_json::to_string_pretty(&raw)?);
                }
            }
        }
        Cmd::Issues => {
            let health = fetch_health(&daemon_addr).await?;
            let history = fetch_history(&daemon_addr).await.ok();
            print_issues(&health, history.as_ref());
        }
        Cmd::History => {
            let history = fetch_history(&daemon_addr).await?;
            print_history(&history);
        }
    }

    Ok(())
}

fn resolve_daemon_addr_from_args(args: &Args) -> DaemonAddr {
    // 1. CLI --socket flag
    if let Some(ref path) = args.socket {
        return DaemonAddr::Unix {
            path: PathBuf::from(path),
        };
    }

    // 2. CLI --url flag
    if let Some(ref url) = args.url {
        if !url.is_empty() {
            return DaemonAddr::Tcp { url: url.clone() };
        }
    }

    // 3. Auto-detect via core (env vars, socket path, port file)
    resolve_daemon_addr()
}

// ── fetch helpers ─────────────────────────────────────────────────────────────

async fn fetch_health(addr: &DaemonAddr) -> Result<HealthResponse> {
    fetch_json(addr, "/health").await
}

async fn fetch_history(addr: &DaemonAddr) -> Result<HistoryResponse> {
    fetch_json(addr, "/history").await
}

async fn fetch_json<T: serde::de::DeserializeOwned>(
    addr: &DaemonAddr,
    endpoint: &str,
) -> Result<T> {
    match addr {
        DaemonAddr::Unix { path } => {
            let body = unix_http_get(path, endpoint).await?;
            serde_json::from_slice(&body)
                .with_context(|| format!("Failed to parse response from {endpoint}"))
        }
        DaemonAddr::Tcp { url } => {
            let client = reqwest::Client::new();
            client
                .get(format!("{url}{endpoint}"))
                .send()
                .await
                .context("Cannot reach daemon")?
                .json::<T>()
                .await
                .with_context(|| format!("Failed to parse response from {endpoint}"))
        }
    }
}

async fn fetch_json_value(addr: &DaemonAddr, endpoint: &str) -> Result<Value> {
    match addr {
        DaemonAddr::Unix { path } => {
            let body = unix_http_get(path, endpoint).await?;
            serde_json::from_slice(&body)
                .with_context(|| format!("Failed to parse response from {endpoint}"))
        }
        DaemonAddr::Tcp { url } => {
            let client = reqwest::Client::new();
            client
                .get(format!("{url}{endpoint}"))
                .send()
                .await
                .context("Cannot reach daemon")?
                .json::<Value>()
                .await
                .with_context(|| format!("Failed to parse response from {endpoint}"))
        }
    }
}

/// Make an HTTP GET request over a Unix domain socket.
async fn unix_http_get(socket_path: &std::path::Path, endpoint: &str) -> Result<Vec<u8>> {
    use http_body_util::{BodyExt, Empty};
    use hyper::{body::Bytes, Request};
    use hyper_util::rt::TokioIo;

    let stream = tokio::net::UnixStream::connect(socket_path)
        .await
        .with_context(|| format!("Failed to connect to daemon at {}", socket_path.display()))?;

    let io = TokioIo::new(stream);
    let (mut sender, conn) = hyper::client::conn::http1::handshake(io)
        .await
        .context("HTTP handshake with daemon failed")?;

    tokio::spawn(conn);

    let request = Request::builder()
        .uri(endpoint)
        .header("Host", "localhost")
        .body(Empty::<Bytes>::new())
        .context("Failed to build HTTP request")?;

    let response = sender
        .send_request(request)
        .await
        .context("Failed to send request to daemon")?;

    if !response.status().is_success() {
        anyhow::bail!("Daemon returned HTTP {}", response.status());
    }

    let collected = response
        .into_body()
        .collect()
        .await
        .context("Failed to read response body")?;

    Ok(collected.to_bytes().to_vec())
}

// ── output formatters ─────────────────────────────────────────────────────────

fn print_human(h: &HealthResponse, history: Option<&HistoryResponse>) {
    let trend = history
        .and_then(|hist| hist.change_24h)
        .map(|d| format!(" ({d:+.1} 24h)"))
        .unwrap_or_default();
    println!("Health: {:.1}/100 ({}){}", h.score, h.status, trend);
    if h.breakdown.errors > 0 || h.breakdown.warnings > 0 {
        println!(
            "Issues: {} error(s), {} warning(s), {} info",
            h.breakdown.errors, h.breakdown.warnings, h.breakdown.info
        );
    }
    if let Some(ref res) = h.resources {
        println!(
            "Resources: CPU {:.1}%, Mem {:.1}%, Load {:.2}",
            res.cpu_usage, res.memory_usage, res.load_average
        );
    }
}

fn print_ironbar(h: &HealthResponse, history: Option<&HistoryResponse>) {
    let class = status_class(&h.status);
    let icon = status_icon(h.score);
    let text = format!("{icon} {:.1}", h.score);

    let mut tooltip_lines: Vec<String> = Vec::new();

    if let Some(hist) = history {
        if !hist.records.is_empty() {
            let sparkline = build_sparkline(&hist.records, 20);
            tooltip_lines.push(format!("Score history (7d): {sparkline}"));
            tooltip_lines.push(String::new());
        }

        let changes = [
            ("1h", hist.change_1h),
            ("24h", hist.change_24h),
            ("7d", hist.change_7d),
        ];
        let change_str: Vec<String> = changes
            .iter()
            .filter_map(|(label, val)| val.map(|v| format!("{label}: {v:+.1}")))
            .collect();
        if !change_str.is_empty() {
            tooltip_lines.push(format!("Change: {}", change_str.join("  ")));
            tooltip_lines.push(String::new());
        }
    }

    if let Some(ref res) = h.resources {
        tooltip_lines.push(format!(
            "CPU {:.1}%  Mem {:.1}%  Load {:.2}",
            res.cpu_usage, res.memory_usage, res.load_average
        ));
        tooltip_lines.push(String::new());
    }

    if h.issues.is_empty() {
        tooltip_lines.push("No active issues".to_string());
    } else {
        tooltip_lines.push("Issues:".to_string());
        for issue in &h.issues {
            tooltip_lines.push(format!(
                "  [{:^7}] {} (×{})  -{:.1}",
                issue.severity.to_uppercase(),
                issue.title,
                issue.count,
                issue.impact
            ));
        }
    }

    let tooltip = tooltip_lines.join("\n");

    let out = serde_json::json!({
        "text": text,
        "tooltip": tooltip,
        "class": class,
        "percentage": (h.score * 10.0).round() / 10.0,
    });
    println!("{out}");
}

fn print_issues(h: &HealthResponse, history: Option<&HistoryResponse>) {
    if h.issues.is_empty() {
        println!("No active issues.\n");
    } else {
        println!(
            "{:<10}  {:>5}  {:<50}  {:>7}",
            "SEVERITY", "COUNT", "TITLE", "BURDEN"
        );
        println!("{}", "-".repeat(80));
        for issue in &h.issues {
            let burden = issue.impact.abs();
            let burden_text = if burden < 0.005 {
                "<0.01".to_string()
            } else {
                format!("{burden:.2}")
            };
            println!(
                "{:<10}  {:>5}  {:<50}  {:>7}",
                issue.severity.to_uppercase(),
                issue.count,
                issue.title,
                burden_text
            );
            if let Some(hint) = issue.hints.first() {
                println!("  {hint:>67}");
            }
        }
    }

    if let Some(ref res) = h.resources {
        let resource_str = format!(
            "CPU {:.1}%  MEM {:.1}%  DISK {:.1}%  LOAD {:.2}",
            res.cpu_usage, res.memory_usage, res.disk_usage, res.load_average
        );
        let burden = res.resource_impact.abs();
        if h.issues.is_empty() {
            println!("{:<10}  {:>5}  {:<50}  {:>7}", "", "", "", "");
        }
        println!(
            "{:<10}  {:>5}  {:<50}  {:>7}",
            "",
            "",
            resource_str,
            format!("{burden:.2}")
        );
        let mut baseline_info = String::new();
        if res.baseline_learning_mode {
            baseline_info = format!(
                "  [baseline: {}/{} learning]",
                res.baseline_samples, res.min_samples_for_baseline
            );
        } else if res.baseline_samples > 0 {
            baseline_info = format!("  [baseline: {} samples]", res.baseline_samples);
        }
        if !baseline_info.is_empty() {
            println!("{baseline_info:>81}");
        }
    }

    println!();
    let change = history.and_then(|hist| hist.change_24h);
    let trend_arrow = match change {
        Some(d) if d > 1.0 => "↗",
        Some(d) if d < -1.0 => "↘",
        Some(_) => "→",
        None => "",
    };
    let trend_text = change
        .map(|d| format!("  ({d:+.1} 24h)"))
        .unwrap_or_default();
    println!("Score {} {:.1}{}", trend_arrow, h.score, trend_text);
    if let Some(hist) = history.filter(|hist| !hist.records.is_empty()) {
        let sparkline = build_sparkline(&hist.records, 32);
        println!("  {sparkline}  (7-day history)");
    }
}

fn print_history(hist: &HistoryResponse) {
    if hist.records.is_empty() {
        println!("No history yet.");
        return;
    }
    let sparkline = build_sparkline(&hist.records, 40);
    println!("7-day score history:");
    println!("  {sparkline}");
    println!();
    if let Some(d) = hist.change_1h {
        println!("  Change  1h:  {d:+.1}");
    }
    if let Some(d) = hist.change_24h {
        println!("  Change 24h:  {d:+.1}");
    }
    if let Some(d) = hist.change_7d {
        println!("  Change  7d:  {d:+.1}");
    }
}

// ── sparkline ─────────────────────────────────────────────────────────────────

const SPARKS: &[char] = &['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];

#[allow(
    clippy::cast_precision_loss,
    clippy::cast_sign_loss,
    clippy::cast_possible_truncation
)]
fn build_sparkline(records: &[HistoryRecord], width: usize) -> String {
    if records.is_empty() {
        return String::new();
    }

    let samples: Vec<f64> = if records.len() <= width {
        records.iter().map(|r| r.score).collect()
    } else {
        let bucket = records.len() / width;
        (0..width)
            .map(|i| {
                let start = i * bucket;
                let end = ((i + 1) * bucket).min(records.len());
                let bucket_size = end - start;
                records[start..end].iter().map(|r| r.score).sum::<f64>() / bucket_size as f64
            })
            .collect()
    };

    let min = samples
        .iter()
        .copied()
        .fold(f64::INFINITY, f64::min)
        .max(0.0);
    let max = samples.iter().copied().fold(0.0_f64, f64::max).min(100.0);
    let range = (max - min).max(1.0);

    samples
        .iter()
        .map(|&v| {
            let idx = ((v - min) / range * (SPARKS.len() - 1) as f64) as usize;
            SPARKS[idx.min(SPARKS.len() - 1)]
        })
        .collect()
}

// ── helpers ───────────────────────────────────────────────────────────────────

fn status_class(status: &str) -> &'static str {
    match status {
        "excellent" => "excellent",
        "good" => "good",
        "fair" => "fair",
        "poor" => "poor",
        _ => "critical",
    }
}

#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn status_icon(score: f64) -> &'static str {
    match score as u32 {
        90..=100 => "\u{f0ed1}", // nerd font heart
        75..=89 => "\u{25cf}",
        50..=74 => "\u{25d5}",
        25..=49 => "\u{25d1}",
        _ => "\u{25cb}",
    }
}
