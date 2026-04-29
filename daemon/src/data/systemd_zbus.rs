//! Real systemd integration using zbus D-Bus interface.
//!
//! This module implements the `SystemdReader` trait using zbus to communicate
//! with systemd over D-Bus, providing real system state information.

use anyhow::{anyhow, Result};
use zbus::{proxy, Connection};

use crate::data::traits::{SystemdReader, SystemdUnit};

/// Type alias for the complex return type of `list_units`
type UnitListResult = zbus::Result<
    Vec<(
        String,
        String,
        String,
        String,
        String,
        String,
        zbus::zvariant::OwnedObjectPath,
        u32,
        String,
        zbus::zvariant::OwnedObjectPath,
    )>,
>;

/// D-Bus proxy interface for systemd Manager
#[proxy(
    interface = "org.freedesktop.systemd1.Manager",
    default_service = "org.freedesktop.systemd1",
    default_path = "/org/freedesktop/systemd1"
)]
trait SystemdManager {
    /// List all units with their current state
    async fn list_units(&self) -> UnitListResult;

    /// Get unit by name
    async fn get_unit(&self, name: &str) -> zbus::Result<zbus::zvariant::OwnedObjectPath>;
}

/// Real systemd reader implementation using zbus
pub struct ZbusSystemdReader {
    connection: Connection,
}

impl ZbusSystemdReader {
    /// Create a new systemd reader with system bus connection
    ///
    /// # Errors
    /// Returns an error if connection to system D-Bus fails
    pub async fn new() -> Result<Self> {
        let connection = Connection::system()
            .await
            .map_err(|e| anyhow!("Failed to connect to system D-Bus: {e}"))?;

        Ok(Self { connection })
    }

    /// Create a new systemd reader with custom connection (for testing)
    #[allow(dead_code)]
    #[must_use]
    pub fn with_connection(connection: Connection) -> Self {
        Self { connection }
    }
}

impl SystemdReader for ZbusSystemdReader {
    async fn list_units(&self) -> Result<Vec<SystemdUnit>> {
        let manager_proxy = SystemdManagerProxy::new(&self.connection)
            .await
            .map_err(|e| anyhow!("Failed to create systemd manager proxy: {e}"))?;

        let units_data = manager_proxy
            .list_units()
            .await
            .map_err(|e| anyhow!("Failed to list systemd units: {e}"))?;

        let mut units = Vec::new();

        for (
            name,
            description,
            load_state,
            active_state,
            sub_state,
            _followed,
            _object_path,
            _job_id,
            _job_type,
            _job_object_path,
        ) in units_data
        {
            // Note: PIDs are intentionally left empty here.
            // Querying GetProcesses via D-Bus for every unit triggers
            // dbus-broker "security policy denied" warnings in the journal for
            // units the calling user cannot inspect — creating a feedback loop
            // where our own queries corrupt the health score.
            // Per-unit process metrics are sourced from procfs (UnitMetricsCollector)
            // which is both cheaper and has no permission issues.
            units.push(SystemdUnit {
                name,
                active_state,
                load_state,
                sub_state,
                description,
                pids: Vec::new(),
            });
        }

        Ok(units)
    }

    async fn get_unit(&self, name: &str) -> Result<Option<SystemdUnit>> {
        let manager_proxy = SystemdManagerProxy::new(&self.connection)
            .await
            .map_err(|e| anyhow!("Failed to create systemd manager proxy: {e}"))?;

        let Ok(_object_path) = manager_proxy.get_unit(name).await else {
            return Ok(None); // Unit not found
        };

        // Get unit properties by listing all units and finding the matching one
        let units = self.list_units().await?;
        Ok(units.into_iter().find(|unit| unit.name == name))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_list_units_integration() {
        // Skip test if D-Bus or systemd is not available
        let Ok(reader) = ZbusSystemdReader::new().await else {
            eprintln!("Skipping systemd D-Bus test: systemd or D-Bus not available");
            return;
        };

        let Ok(units) = reader.list_units().await else {
            eprintln!("Skipping systemd D-Bus test: failed to list units");
            return;
        };

        // Should have at least some units on a running system
        assert!(!units.is_empty());

        // Check that we have some common expected properties
        for unit in &units[..std::cmp::min(5, units.len())] {
            assert!(!unit.name.is_empty());
            assert!(!unit.active_state.is_empty());
            assert!(!unit.load_state.is_empty());
            assert!(!unit.sub_state.is_empty());
        }
    }

    #[tokio::test]
    async fn test_get_unit_integration() {
        // Skip test if D-Bus or systemd is not available
        let Ok(reader) = ZbusSystemdReader::new().await else {
            eprintln!("Skipping systemd D-Bus test: systemd or D-Bus not available");
            return;
        };

        // Try to get a unit that should exist on most systems
        if let Ok(Some(unit)) = reader.get_unit("dbus.service").await {
            assert_eq!(unit.name, "dbus.service");
            assert!(!unit.active_state.is_empty());
        }

        // Try to get a non-existent unit
        let result = reader.get_unit("non-existent-unit.service").await;
        assert!(result.is_ok());
        assert!(result.unwrap().is_none());
    }
}
