//! Configuration management for vitals-daemon.
//!
//! Handles loading and saving configuration from ~/.config/vitals/config.toml

use std::{
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result};
use scorer::TwhsConfig;
use serde::{Deserialize, Serialize};

use crate::{agg::AggregationConfig, notifier::NotifierConfig};

/// Main configuration structure
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Config {
    /// TWHS algorithm configuration
    #[serde(default)]
    pub twhs: TwhsConfig,
    /// Daemon configuration
    #[serde(default)]
    pub daemon: DaemonConfig,
    /// Metrics collection configuration
    #[serde(default)]
    pub metrics: MetricsConfig,
    /// Issue aggregation configuration
    #[serde(default)]
    pub aggregation: AggregationConfig,
    /// Notifier configuration
    #[serde(default)]
    pub notifier: NotifierConfig,
}

/// Daemon-specific configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DaemonConfig {
    /// Port to bind the web server to
    pub port: u16,
    /// Host to bind to
    pub host: String,
    /// Health calculation interval in seconds
    pub calculation_interval: u64,
    /// Maximum number of journal entries to process
    pub max_journal_entries: usize,
    /// Time window for journal entries (in hours)
    pub journal_time_window_hours: u64,
}

impl Default for DaemonConfig {
    fn default() -> Self {
        Self {
            port: 8080,
            host: "127.0.0.1".to_string(),
            calculation_interval: 30,
            max_journal_entries: 1000,
            journal_time_window_hours: 24,
        }
    }
}

/// Metrics collection configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetricsConfig {
    /// Sampling interval in seconds
    pub sample_interval_secs: u64,
    /// Number of samples to keep in memory (default: 300 for 5 minutes at 1s)
    pub memory_buffer_size: usize,
    /// Whether to enable persistent storage
    pub enable_persistence: bool,
    /// Path for persistent storage (optional)
    pub persistence_path: Option<PathBuf>,
    /// Whether to enable per-process metrics collection
    pub enable_process_metrics: bool,
    /// Whether to enable per-unit metrics aggregation
    pub enable_unit_metrics: bool,
}

impl Default for MetricsConfig {
    fn default() -> Self {
        Self {
            sample_interval_secs: 1,
            memory_buffer_size: 300,
            enable_persistence: false,
            persistence_path: None,
            enable_process_metrics: false,
            enable_unit_metrics: true,
        }
    }
}

impl Config {
    /// Load configuration from the default path
    ///
    /// # Errors
    ///
    /// Returns an error if the file cannot be read or parsed.
    pub fn load() -> Result<Self> {
        let config_path = Self::default_path()?;
        Self::load_from_path(&config_path)
    }

    /// Load configuration from a specific path
    ///
    /// # Errors
    ///
    /// Returns an error if the file cannot be read or parsed.
    pub fn load_from_path(path: &Path) -> Result<Self> {
        if !path.exists() {
            return Ok(Self::default());
        }

        let contents = fs::read_to_string(path)
            .with_context(|| format!("Failed to read config from {}", path.display()))?;

        let config: Config = toml::from_str(&contents)
            .with_context(|| format!("Failed to parse config from {}", path.display()))?;

        Ok(config)
    }

    /// Save configuration to the default path
    ///
    /// # Errors
    ///
    /// Returns an error if the path cannot be written to.
    pub fn save(&self) -> Result<()> {
        let config_path = Self::default_path()?;
        self.save_to_path(&config_path)
    }

    /// Save configuration to a specific path
    ///
    /// # Errors
    ///
    /// Returns an error if the path cannot be written to.
    pub fn save_to_path(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).with_context(|| {
                format!("Failed to create config directory {}", parent.display())
            })?;
        }

        let contents =
            toml::to_string_pretty(self).context("Failed to serialize configuration to TOML")?;

        fs::write(path, contents)
            .with_context(|| format!("Failed to write config to {}", path.display()))?;

        Ok(())
    }

    /// Get the default configuration path
    fn default_path() -> Result<PathBuf> {
        let config_dir = dirs::config_dir()
            .context("Could not determine user config directory")?
            .join("vitals");

        Ok(config_dir.join("config.toml"))
    }
}

/// Validate configuration values
///
/// # Errors
///
/// Returns an error if any configuration value is invalid.
pub fn validate_config(config: &Config) -> Result<()> {
    if config.daemon.port == 0 {
        anyhow::bail!("Daemon port must be non-zero");
    }

    if config.daemon.calculation_interval == 0 {
        anyhow::bail!("Calculation interval must be non-zero");
    }

    if config.twhs.sensitivity <= 0.0 {
        anyhow::bail!("TWHS sensitivity (κ) must be positive");
    }

    if config.twhs.decay_half_life_hours <= 0.0 {
        anyhow::bail!("TWHS decay_half_life_hours must be positive");
    }

    if config.notifier.alert_below < 0.0 || config.notifier.alert_below > 100.0 {
        anyhow::bail!("Notifier alert_below threshold must be between 0 and 100");
    }

    if config.notifier.cooldown_secs == 0 {
        anyhow::bail!("Notifier cooldown_secs must be non-zero");
    }

    Ok(())
}
