use std::path::PathBuf;

#[derive(Clone, Debug)]
pub enum DaemonAddr {
    Unix { path: PathBuf },
    Tcp { url: String },
}

/// Resolve the daemon address with the following priority:
/// 1. Unix socket at `$XDG_RUNTIME_DIR/vitals/daemon.sock` (primary transport)
/// 2. `VITALS_URL` env var (backward compat for TCP setups)
/// 3. Port file at `$XDG_STATE_HOME/vitals/addr` (written by daemon)
/// 4. Default `http://127.0.0.1:8080`
#[must_use]
pub fn resolve_daemon_addr() -> DaemonAddr {
    let socket_path = daemon_socket_path();
    if socket_path.exists() {
        return DaemonAddr::Unix { path: socket_path };
    }

    if let Ok(url) = std::env::var("VITALS_URL") {
        if !url.is_empty() {
            return DaemonAddr::Tcp { url };
        }
    }

    if let Some(port_file) = state_file_path() {
        if let Ok(url) = std::fs::read_to_string(&port_file) {
            let url = url.trim().to_string();
            if !url.is_empty() {
                return DaemonAddr::Tcp { url };
            }
        }
    }

    DaemonAddr::Tcp {
        url: "http://127.0.0.1:8080".to_string(),
    }
}

/// Standard Unix socket path for the vitals daemon.
#[must_use]
pub fn daemon_socket_path() -> PathBuf {
    if let Ok(dir) = std::env::var("XDG_RUNTIME_DIR") {
        PathBuf::from(dir).join("vitals").join("daemon.sock")
    } else if let Ok(tmpdir) = std::env::var("TMPDIR") {
        PathBuf::from(tmpdir).join("vitals").join("daemon.sock")
    } else {
        PathBuf::from("/tmp/vitals/daemon.sock")
    }
}

fn state_file_path() -> Option<PathBuf> {
    if let Ok(dir) = std::env::var("XDG_STATE_HOME") {
        Some(PathBuf::from(dir).join("vitals").join("addr"))
    } else if let Ok(home) = std::env::var("HOME") {
        Some(
            PathBuf::from(home)
                .join(".local")
                .join("state")
                .join("vitals")
                .join("addr"),
        )
    } else {
        None
    }
}

/// Write the daemon address to a state file so the CLI can discover it.
pub fn write_addr_file(addr: &DaemonAddr) {
    let content = match addr {
        DaemonAddr::Unix { path } => format!("unix:{}", path.display()),
        DaemonAddr::Tcp { url } => url.clone(),
    };
    if let Some(path) = state_file_path() {
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::write(&path, content);
    }
}
