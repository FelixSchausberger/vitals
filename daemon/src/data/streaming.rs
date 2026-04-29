//! Streaming log updates with configurable intervals.
//!
//! This module provides efficient streaming of journal entries with configurable
//! update intervals, inspired by nerdlog and lazyjournal's real-time capabilities.

#![allow(dead_code)]

use std::time::{Duration, Instant};

use tokio::{
    sync::mpsc,
    time::{interval, MissedTickBehavior},
};
use tokio_stream::{wrappers::IntervalStream, StreamExt};

use super::traits::JournalEntry;

/// Configuration for streaming log updates
#[derive(Debug, Clone)]
pub struct StreamConfig {
    /// Update interval in milliseconds
    pub update_interval_ms: u64,
    /// Maximum number of entries to fetch per update
    pub max_entries_per_update: usize,
    /// Whether to enable streaming (can be disabled for performance)
    pub enabled: bool,
    /// Buffer size for the channel between stream and UI
    pub channel_buffer_size: usize,
    /// Whether to include debug-level entries
    pub include_debug: bool,
    /// Minimum priority level to include (0-7, systemd priority levels)
    pub min_priority: u8,
}

impl Default for StreamConfig {
    fn default() -> Self {
        Self {
            update_interval_ms: 1000,    // 1 second default
            max_entries_per_update: 100, // Reasonable batch size
            enabled: true,
            channel_buffer_size: 1000, // Buffer up to 1000 updates
            include_debug: false,      // Skip debug entries by default
            min_priority: 6,           // Include info and above (0-6)
        }
    }
}

/// Stream update containing new journal entries
#[derive(Debug, Clone)]
pub struct StreamUpdate {
    /// New journal entries since last update
    pub entries: Vec<JournalEntry>,
    /// Timestamp when this update was generated
    pub timestamp: Instant,
    /// Whether this is an initial load or incremental update
    pub is_initial: bool,
    /// Number of entries that were filtered out
    pub filtered_count: usize,
}

/// Stream state for tracking what's been sent
#[derive(Debug)]
struct StreamState {
    /// Last timestamp we've processed
    last_timestamp: Option<time::OffsetDateTime>,
    /// Configuration for this stream
    config: StreamConfig,
    /// Whether we've sent the initial batch
    initial_sent: bool,
}

impl StreamState {
    fn new(config: StreamConfig) -> Self {
        Self {
            last_timestamp: None,
            config,
            initial_sent: false,
        }
    }
}

/// Streaming journal reader that provides real-time updates
pub struct StreamingJournal {
    /// Channel receiver for stream updates
    pub updates: mpsc::Receiver<StreamUpdate>,
    /// Handle to the background streaming task
    #[allow(dead_code)]
    task_handle: tokio::task::JoinHandle<()>,
}

impl StreamingJournal {
    /// Create a new streaming journal with the given configuration
    pub fn new(
        config: &StreamConfig,
        data_fetcher: impl Fn() -> Result<Vec<JournalEntry>, Box<dyn std::error::Error + Send + Sync>>
            + Send
            + Sync
            + 'static,
    ) -> Self {
        let (tx, rx) = mpsc::channel(config.channel_buffer_size);
        let mut state = StreamState::new(config.clone());

        let task_handle = tokio::spawn(async move {
            if !state.config.enabled {
                return;
            }

            let mut interval = interval(Duration::from_millis(state.config.update_interval_ms));
            interval.set_missed_tick_behavior(MissedTickBehavior::Skip);

            let mut interval_stream = IntervalStream::new(interval);

            while interval_stream.next().await.is_some() {
                match Self::fetch_update(&mut state, &data_fetcher) {
                    Ok(Some(update)) => {
                        if tx.send(update).await.is_err() {
                            // Receiver dropped, exit the stream
                            break;
                        }
                    }
                    Ok(None) => {
                        // No new data, continue
                    }
                    Err(e) => {
                        log::warn!("Failed to fetch journal update: {e}");
                        // Continue on errors to maintain streaming
                    }
                }
            }
        });

        Self {
            updates: rx,
            task_handle,
        }
    }

    /// Fetch a single update from the data source
    fn fetch_update(
        state: &mut StreamState,
        data_fetcher: &impl Fn() -> Result<Vec<JournalEntry>, Box<dyn std::error::Error + Send + Sync>>,
    ) -> Result<Option<StreamUpdate>, Box<dyn std::error::Error + Send + Sync>> {
        let all_entries = data_fetcher()?;

        let now = Instant::now();
        let is_initial = !state.initial_sent;

        if is_initial {
            // Initial load: return recent entries up to the limit
            let recent_entries = Self::filter_entries(&all_entries, &state.config);
            let limited_entries = if recent_entries.len() > state.config.max_entries_per_update {
                // Take the most recent entries
                recent_entries
                    .into_iter()
                    .rev()
                    .take(state.config.max_entries_per_update)
                    .rev()
                    .collect()
            } else {
                recent_entries
            };

            // Update state
            if let Some(last_entry) = limited_entries.last() {
                state.last_timestamp = Some(last_entry.timestamp);
            }
            state.initial_sent = true;

            let filtered_count = all_entries.len().saturating_sub(limited_entries.len());

            return Ok(Some(StreamUpdate {
                entries: limited_entries,
                timestamp: now,
                is_initial: true,
                filtered_count,
            }));
        }

        // Incremental update: only return new entries since last timestamp
        let new_entries = if let Some(last_ts) = state.last_timestamp {
            all_entries
                .into_iter()
                .filter(|entry| entry.timestamp > last_ts)
                .collect::<Vec<_>>()
        } else {
            all_entries
        };

        if new_entries.is_empty() {
            return Ok(None);
        }

        let filtered_entries = Self::filter_entries(&new_entries, &state.config);
        let limited_entries = if filtered_entries.len() > state.config.max_entries_per_update {
            // Take the most recent entries
            filtered_entries
                .into_iter()
                .rev()
                .take(state.config.max_entries_per_update)
                .rev()
                .collect()
        } else {
            filtered_entries
        };

        // Update last timestamp
        if let Some(last_entry) = limited_entries.last() {
            state.last_timestamp = Some(last_entry.timestamp);
        }

        let filtered_count = new_entries.len().saturating_sub(limited_entries.len());

        Ok(Some(StreamUpdate {
            entries: limited_entries,
            timestamp: now,
            is_initial: false,
            filtered_count,
        }))
    }

    /// Filter entries based on configuration
    fn filter_entries(entries: &[JournalEntry], config: &StreamConfig) -> Vec<JournalEntry> {
        entries
            .iter()
            .filter(|entry| {
                // Filter by priority level
                if entry.priority > config.min_priority {
                    return false;
                }

                // Filter debug entries if disabled
                if !config.include_debug && entry.priority >= 7 {
                    return false;
                }

                true
            })
            .cloned()
            .collect()
    }

    /// Get the next update from the stream
    pub async fn next_update(&mut self) -> Option<StreamUpdate> {
        self.updates.recv().await
    }

    /// Check if there are any pending updates without waiting
    pub fn try_next_update(&mut self) -> Option<StreamUpdate> {
        self.updates.try_recv().ok()
    }
}

/// Builder for creating streaming journal configurations
#[derive(Debug, Clone)]
pub struct StreamConfigBuilder {
    config: StreamConfig,
}

impl Default for StreamConfigBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl StreamConfigBuilder {
    /// Create a new builder with default configuration
    #[must_use]
    pub fn new() -> Self {
        Self {
            config: StreamConfig::default(),
        }
    }

    /// Set the update interval in milliseconds
    #[must_use]
    pub fn update_interval_ms(mut self, interval_ms: u64) -> Self {
        self.config.update_interval_ms = interval_ms;
        self
    }

    /// Set the maximum entries per update
    #[must_use]
    pub fn max_entries_per_update(mut self, max_entries: usize) -> Self {
        self.config.max_entries_per_update = max_entries;
        self
    }

    /// Enable or disable streaming
    #[must_use]
    pub fn enabled(mut self, enabled: bool) -> Self {
        self.config.enabled = enabled;
        self
    }

    /// Set the channel buffer size
    #[must_use]
    pub fn channel_buffer_size(mut self, buffer_size: usize) -> Self {
        self.config.channel_buffer_size = buffer_size;
        self
    }

    /// Include or exclude debug entries
    #[must_use]
    pub fn include_debug(mut self, include_debug: bool) -> Self {
        self.config.include_debug = include_debug;
        self
    }

    /// Set minimum priority level (0-7, systemd levels)
    #[must_use]
    pub fn min_priority(mut self, min_priority: u8) -> Self {
        self.config.min_priority = min_priority.min(7); // Cap at 7
        self
    }

    /// Build the configuration
    #[must_use]
    pub fn build(self) -> StreamConfig {
        self.config
    }
}

/// Preset configurations for common use cases
impl StreamConfig {
    /// High-frequency configuration for development/debugging
    #[must_use]
    pub fn high_frequency() -> Self {
        StreamConfigBuilder::new()
            .update_interval_ms(500) // 0.5 seconds
            .max_entries_per_update(50)
            .include_debug(true)
            .min_priority(7) // Include all priorities
            .build()
    }

    /// Low-frequency configuration for production monitoring
    #[must_use]
    pub fn low_frequency() -> Self {
        StreamConfigBuilder::new()
            .update_interval_ms(5000) // 5 seconds
            .max_entries_per_update(200)
            .include_debug(false)
            .min_priority(4) // Warning and above
            .build()
    }

    /// Performance-optimized configuration for large systems
    #[must_use]
    pub fn performance_optimized() -> Self {
        StreamConfigBuilder::new()
            .update_interval_ms(2000) // 2 seconds
            .max_entries_per_update(100)
            .include_debug(false)
            .min_priority(3) // Error and above
            .channel_buffer_size(500) // Smaller buffer
            .build()
    }

    /// Disabled configuration (no streaming)
    #[must_use]
    pub fn disabled() -> Self {
        StreamConfigBuilder::new().enabled(false).build()
    }
}

#[cfg(test)]
mod tests {
    use time::OffsetDateTime;

    use super::*;

    fn create_test_entry(priority: u8, message: &str) -> JournalEntry {
        JournalEntry {
            timestamp: OffsetDateTime::now_utc(),
            priority,
            message: message.to_string(),
            unit: Some("test.service".to_string()),
            pid: Some(1234),
        }
    }

    #[test]
    fn test_stream_config_builder() {
        let config = StreamConfigBuilder::new()
            .update_interval_ms(2000)
            .max_entries_per_update(50)
            .include_debug(true)
            .min_priority(5)
            .build();

        assert_eq!(config.update_interval_ms, 2000);
        assert_eq!(config.max_entries_per_update, 50);
        assert!(config.include_debug);
        assert_eq!(config.min_priority, 5);
    }

    #[test]
    fn test_filter_entries() {
        let entries = vec![
            create_test_entry(0, "emergency"), // Should be included
            create_test_entry(3, "error"),     // Should be included
            create_test_entry(6, "info"),      // Should be included
            create_test_entry(7, "debug"),     // Should be filtered out
        ];

        let config = StreamConfig::default(); // min_priority = 6, include_debug = false
        let filtered = StreamingJournal::filter_entries(&entries, &config);

        assert_eq!(filtered.len(), 3); // Emergency, error, info
        assert!(filtered.iter().all(|e| e.priority <= 6));
    }

    #[test]
    fn test_preset_configurations() {
        let high_freq = StreamConfig::high_frequency();
        assert_eq!(high_freq.update_interval_ms, 500);
        assert!(high_freq.include_debug);

        let low_freq = StreamConfig::low_frequency();
        assert_eq!(low_freq.update_interval_ms, 5000);
        assert!(!low_freq.include_debug);

        let disabled = StreamConfig::disabled();
        assert!(!disabled.enabled);
    }

    #[tokio::test]
    async fn test_streaming_journal_creation() {
        let config = StreamConfig::performance_optimized();
        let data_fetcher = || Ok(vec![create_test_entry(6, "test message")]);

        let mut stream = StreamingJournal::new(&config, data_fetcher);

        // Should be able to create without errors
        assert!(stream.try_next_update().is_none()); // No immediate updates
    }
}
