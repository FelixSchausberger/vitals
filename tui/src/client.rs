//! HTTP client for vitals daemon API.

use anyhow::{Context, Result};
use vitals_core::api::{HealthResponse, LogsResponse};

/// Client for interacting with the vitals daemon API
#[derive(Clone)]
pub struct DaemonClient {
    base_url: String,
    client: reqwest::Client,
}

impl DaemonClient {
    /// Create a new daemon client
    pub fn new(base_url: &str) -> Result<Self> {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(5))
            .build()
            .context("Failed to create HTTP client")?;

        Ok(Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            client,
        })
    }

    /// Fetch health data from /health endpoint
    pub async fn get_health(&self) -> Result<HealthResponse> {
        let url = format!("{}/health", self.base_url);
        let response = self
            .client
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

    /// Fetch logs from /logs endpoint
    #[allow(dead_code)]
    pub async fn get_logs(&self) -> Result<LogsResponse> {
        let url = format!("{}/logs", self.base_url);
        let response = self
            .client
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

    /// Check if daemon is reachable
    #[allow(dead_code)]
    pub async fn ping(&self) -> bool {
        let url = format!("{}/health", self.base_url);
        self.client
            .get(&url)
            .send()
            .await
            .map(|r| r.status().is_success())
            .unwrap_or(false)
    }
}
