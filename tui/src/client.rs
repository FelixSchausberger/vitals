use std::path::PathBuf;

use anyhow::{Context, Result};
use url::form_urlencoded;
use vitals_core::{
    addr::DaemonAddr,
    api::{HealthResponse, LogsQuery, LogsResponse, SeverityFilter},
};

/// Client for interacting with the vitals daemon API
#[derive(Clone)]
pub struct DaemonClient {
    addr: DaemonAddr,
}

impl DaemonClient {
    /// Create a new daemon client from a resolved address
    pub fn from_addr(addr: DaemonAddr) -> Self {
        Self { addr }
    }

    /// Create a new daemon client from a TCP base URL
    #[allow(dead_code)]
    pub fn new(base_url: &str) -> Self {
        Self {
            addr: DaemonAddr::Tcp {
                url: base_url.trim_end_matches('/').to_string(),
            },
        }
    }

    /// Fetch health data from /health endpoint
    pub async fn get_health(&self) -> Result<HealthResponse> {
        match &self.addr {
            DaemonAddr::Unix { path } => {
                let body = unix_http_get(path, "/health").await?;
                serde_json::from_slice(&body).context("Failed to parse health response")
            }
            DaemonAddr::Tcp { url } => {
                let client = reqwest::Client::builder()
                    .timeout(std::time::Duration::from_secs(5))
                    .build()
                    .context("Failed to create HTTP client")?;
                let url = format!("{url}/health");
                let response = client
                    .get(&url)
                    .send()
                    .await
                    .context("Failed to send health request")?;
                if !response.status().is_success() {
                    anyhow::bail!("Health endpoint returned status: {}", response.status());
                }
                response
                    .json()
                    .await
                    .context("Failed to parse health response")
            }
        }
    }

    /// Fetch logs from /logs endpoint with optional filtering/pagination
    pub async fn get_logs(&self, query: &LogsQuery) -> Result<LogsResponse> {
        let qs = encode_query(query);
        let endpoint = if qs.is_empty() {
            "/logs".to_string()
        } else {
            format!("/logs?{qs}")
        };
        match &self.addr {
            DaemonAddr::Unix { path } => {
                let body = unix_http_get(path, &endpoint).await?;
                serde_json::from_slice(&body).context("Failed to parse logs response")
            }
            DaemonAddr::Tcp { url } => {
                let client = reqwest::Client::builder()
                    .timeout(std::time::Duration::from_secs(5))
                    .build()
                    .context("Failed to create HTTP client")?;
                let url = format!("{url}{endpoint}");
                let response = client
                    .get(&url)
                    .send()
                    .await
                    .context("Failed to send logs request")?;
                if !response.status().is_success() {
                    anyhow::bail!("Logs endpoint returned status: {}", response.status());
                }
                response
                    .json()
                    .await
                    .context("Failed to parse logs response")
            }
        }
    }

    /// Check if daemon is reachable
    #[allow(dead_code)]
    pub async fn ping(&self) -> bool {
        match &self.addr {
            DaemonAddr::Unix { path } => tokio::net::UnixStream::connect(path)
                .await
                .map(|_| true)
                .unwrap_or(false),
            DaemonAddr::Tcp { url } => {
                let client = reqwest::Client::builder()
                    .timeout(std::time::Duration::from_secs(2))
                    .build()
                    .ok();
                match client {
                    Some(c) => c
                        .get(format!("{url}/health"))
                        .send()
                        .await
                        .map(|r| r.status().is_success())
                        .unwrap_or(false),
                    None => false,
                }
            }
        }
    }
}

/// Encode a [`LogsQuery`] as URL query parameters (no leading `?`).
fn encode_query(query: &LogsQuery) -> String {
    let mut serializer = form_urlencoded::Serializer::new(String::new());
    if let Some(severity) = query.severity {
        let name = match severity {
            SeverityFilter::Error => "error",
            SeverityFilter::Warning => "warning",
            SeverityFilter::Info => "info",
        };
        serializer.append_pair("severity", name);
    }
    if let Some(ref unit) = query.unit {
        serializer.append_pair("unit", unit);
    }
    if let Some(since) = query.since {
        serializer.append_pair("since", &format_rfc3339(since));
    }
    if let Some(until) = query.until {
        serializer.append_pair("until", &format_rfc3339(until));
    }
    if let Some(limit) = query.limit {
        serializer.append_pair("limit", &limit.to_string());
    }
    if query.offset != 0 {
        serializer.append_pair("offset", &query.offset.to_string());
    }
    serializer.finish()
}

/// Format a timestamp as RFC 3339 for use in query parameters.
fn format_rfc3339(ts: time::OffsetDateTime) -> String {
    ts.format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_else(|_| String::from("invalid-timestamp"))
}

/// Make an HTTP GET request over a Unix domain socket.
async fn unix_http_get(socket_path: &PathBuf, endpoint: &str) -> Result<Vec<u8>> {
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
