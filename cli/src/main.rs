//! Vitals CLI — query the daemon and report system health.
//!
//! Output formats:
//!   human    — readable one-liner (default)
//!   ironbar  — waybar-compatible JSON for ironbar/waybar widgets
//!   json     — raw daemon JSON pass-through

use anyhow::{Context, Result};
use clap::{Parser, Subcommand, ValueEnum};
use serde::Deserialize;
use serde_json::Value;

// ── CLI args ──────────────────────────────────────────────────────────────────

#[derive(Parser, Debug)]
#[command(
    name = "vitals",
    about = "Query the vitals health daemon",
    version = "0.1.0"
)]
struct Args {
    /// Daemon base URL
    #[arg(long, env = "VITALS_URL", default_value = "http://127.0.0.1:8080")]
    url: String,

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
    #[serde(default)]
    raw_score: f64,
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

#[derive(Debug, Deserialize)]
struct IssueImpact {
    title: String,
    severity: String,
    count: usize,
    impact: f64,
}

#[derive(Debug, Deserialize)]
struct Resources {
    cpu_usage: f64,
    memory_usage: f64,
    load_average: f64,
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
    let client = reqwest::Client::new();

    match args.command {
        Cmd::Status { format } => {
            let health = fetch_health(&client, &args.url).await?;
            let history = fetch_history(&client, &args.url).await.ok();
            match format {
                Format::Human => print_human(&health, history.as_ref()),
                Format::Ironbar => print_ironbar(&health, history.as_ref()),
                Format::Json => {
                    let raw: Value = client
                        .get(format!("{}/health", args.url))
                        .send()
                        .await?
                        .json()
                        .await?;
                    println!("{}", serde_json::to_string_pretty(&raw)?);
                }
            }
        }
        Cmd::Issues => {
            let health = fetch_health(&client, &args.url).await?;
            print_issues(&health);
        }
        Cmd::History => {
            let history = fetch_history(&client, &args.url).await?;
            print_history(&history);
        }
    }

    Ok(())
}

// ── fetch helpers ─────────────────────────────────────────────────────────────

async fn fetch_health(client: &reqwest::Client, base: &str) -> Result<HealthResponse> {
    client
        .get(format!("{base}/health"))
        .send()
        .await
        .context("Cannot reach daemon")?
        .json::<HealthResponse>()
        .await
        .context("Failed to parse health response")
}

async fn fetch_history(client: &reqwest::Client, base: &str) -> Result<HistoryResponse> {
    client
        .get(format!("{base}/history"))
        .send()
        .await
        .context("Cannot reach daemon")?
        .json::<HistoryResponse>()
        .await
        .context("Failed to parse history response")
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

/// Ironbar / waybar-compatible JSON widget output.
///
/// Schema:
/// ```json
/// { "text": "● 88.5", "tooltip": "<details>", "class": "good", "percentage": 88.5 }
/// ```
fn print_ironbar(h: &HealthResponse, history: Option<&HistoryResponse>) {
    let class = status_class(&h.status);
    let icon = status_icon(h.score);
    let text = format!("{icon} {:.1}", h.score);

    // Build tooltip: sparkline + change indicators + issue list
    let mut tooltip_lines: Vec<String> = Vec::new();

    // Sparkline from history
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

    // Resource summary
    if let Some(ref res) = h.resources {
        tooltip_lines.push(format!(
            "CPU {:.1}%  Mem {:.1}%  Load {:.2}",
            res.cpu_usage, res.memory_usage, res.load_average
        ));
        tooltip_lines.push(String::new());
    }

    // Issues
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

    // Escape for JSON string (newlines → \n literal is fine for waybar/ironbar)
    let out = serde_json::json!({
        "text": text,
        "tooltip": tooltip,
        "class": class,
        "percentage": h.score,
    });
    println!("{out}");
}

fn print_issues(h: &HealthResponse) {
    if h.issues.is_empty() {
        println!("No active issues.");
        return;
    }
    println!(
        "{:<10}  {:<6}  {:<50}  IMPACT",
        "SEVERITY", "COUNT", "TITLE"
    );
    println!("{}", "-".repeat(80));
    for issue in &h.issues {
        println!(
            "{:<10}  {:<6}  {:<50}  -{:.2}",
            issue.severity.to_uppercase(),
            issue.count,
            issue.title,
            issue.impact
        );
    }
    println!();
    let total: f64 = h.issues.iter().map(|i| i.impact).sum();
    println!(
        "Raw score: {:.1}  Smoothed: {:.1}  Total burden: {:.2}",
        h.raw_score, h.score, total
    );
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

    // Downsample or use all records if fewer than width
    let samples: Vec<f64> = if records.len() <= width {
        records.iter().map(|r| r.score).collect()
    } else {
        // Average bucket sampling
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
        90..=100 => "󰋑", // nerd font heart
        75..=89 => "●",
        50..=74 => "◕",
        25..=49 => "◑",
        _ => "○",
    }
}
