pub mod mock;
pub mod parser;
pub mod streaming;
pub mod traits;

// Real adapters
#[cfg(feature = "systemd")]
pub mod journal_sd;
pub mod metrics_collector;
pub mod metrics_procfs;
pub mod metrics_system;
#[cfg(feature = "systemd")]
pub mod systemd_zbus;
