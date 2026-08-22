//! TUI configuration loaded from `~/.config/vitals/tui.toml`.
//!
//! All fields are optional; a missing file yields defaults. CLI arguments
//! take precedence over values set here.

use std::{
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

/// Persistent TUI settings.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TuiConfig {
    /// Daemon base URL for TCP connections (e.g. `http://localhost:8080`)
    pub daemon_url: Option<String>,
    /// Daemon Unix socket path (takes precedence over `daemon_url`)
    pub daemon_socket: Option<String>,
    /// Refresh interval in seconds
    pub refresh_secs: Option<u64>,
    /// View shown on startup: "summary", "detailed", or "logs"
    pub default_view: Option<String>,
}

impl TuiConfig {
    /// Load configuration from the default path.
    ///
    /// # Errors
    ///
    /// Returns an error if the file exists but cannot be read or parsed.
    pub fn load() -> Result<Self> {
        let path = Self::default_path()?;
        Self::load_from_path(&path)
    }

    /// Load configuration from a specific path.
    ///
    /// # Errors
    ///
    /// Returns an error if the file exists but cannot be read or parsed.
    pub fn load_from_path(path: &Path) -> Result<Self> {
        if !path.exists() {
            return Ok(Self::default());
        }

        let contents = fs::read_to_string(path)
            .with_context(|| format!("Failed to read config from {}", path.display()))?;

        let config: Self = toml::from_str(&contents)
            .with_context(|| format!("Failed to parse config from {}", path.display()))?;

        Ok(config)
    }

    /// Default configuration path: `~/.config/vitals/tui.toml`.
    fn default_path() -> Result<PathBuf> {
        let dir = dirs::config_dir()
            .context("Could not determine user config directory")?
            .join("vitals");
        Ok(dir.join("tui.toml"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_file_yields_defaults() {
        let config = TuiConfig::load_from_path(Path::new("/nonexistent/vitals/tui.toml"))
            .expect("missing file is not an error");
        assert_eq!(config, TuiConfig::default());
    }

    #[test]
    fn parses_all_fields() {
        let dir = std::env::temp_dir().join(format!("vitals-tui-test-{}", std::process::id()));
        fs::create_dir_all(&dir).expect("mkdir");
        let path = dir.join("tui.toml");
        fs::write(
            &path,
            "daemon_url = \"http://localhost:9999\"\n\
             refresh_secs = 5\n\
             default_view = \"logs\"\n",
        )
        .expect("write");

        let config = TuiConfig::load_from_path(&path).expect("parse");
        assert_eq!(config.daemon_url.as_deref(), Some("http://localhost:9999"));
        assert_eq!(config.refresh_secs, Some(5));
        assert_eq!(config.default_view.as_deref(), Some("logs"));
        assert!(config.daemon_socket.is_none());

        fs::remove_file(&path).ok();
    }

    #[test]
    fn malformed_file_is_an_error() {
        let dir = std::env::temp_dir().join(format!("vitals-tui-bad-{}", std::process::id()));
        fs::create_dir_all(&dir).expect("mkdir");
        let path = dir.join("bad.toml");
        fs::write(&path, "refresh_secs = \"not-a-number\"\n").expect("write");

        assert!(TuiConfig::load_from_path(&path).is_err());

        fs::remove_file(&path).ok();
    }
}
