use std::path::PathBuf;

#[derive(Clone, Debug)]
pub enum DaemonAddr {
    Unix { path: PathBuf },
    Tcp { url: String },
}

/// Canonical Unix socket path a vitals daemon binds to.
///
/// `$XDG_RUNTIME_DIR/vitals/daemon.sock` — i.e. `/run/vitals/daemon.sock` for a
/// system service (the NixOS module sets `XDG_RUNTIME_DIR=/run` alongside
/// `RuntimeDirectory=vitals`), `/run/user/<uid>/vitals/daemon.sock` for a
/// per-user service, and `$TMPDIR/vitals/daemon.sock` when no runtime dir
/// exists at all.
#[must_use]
pub fn daemon_socket_path() -> PathBuf {
    if let Ok(dir) = std::env::var("XDG_RUNTIME_DIR") {
        if !dir.is_empty() {
            return PathBuf::from(dir).join("vitals").join("daemon.sock");
        }
    }

    if let Ok(tmpdir) = std::env::var("TMPDIR") {
        if !tmpdir.is_empty() {
            return PathBuf::from(tmpdir).join("vitals").join("daemon.sock");
        }
    }

    PathBuf::from("/tmp/vitals/daemon.sock")
}

/// Socket paths a client probes, most specific first.
///
/// A client never shares the environment of a *system* daemon, so its own
/// `$XDG_RUNTIME_DIR` cannot reach `/run/vitals/daemon.sock`. The system path
/// is therefore listed explicitly rather than derived from the environment.
fn client_socket_candidates() -> Vec<PathBuf> {
    let mut paths = vec![daemon_socket_path()];

    let system = PathBuf::from("/run/vitals/daemon.sock");
    if !paths.contains(&system) {
        paths.push(system);
    }

    let shared = PathBuf::from("/tmp/vitals/daemon.sock");
    if !paths.contains(&shared) {
        paths.push(shared);
    }

    paths
}

/// Resolve the address a client should connect to.
///
/// 1. The first socket that actually accepts a connection. `exists()` is not
///    enough: a crashed daemon leaves a stale socket behind, and a system
///    daemon's socket is not writable by other users (`DynamicUser` + the
///    service umask), so a probe rejects candidates this process cannot use.
/// 2. `VITALS_URL`.
/// 3. `http://127.0.0.1:8080`.
#[must_use]
pub fn resolve_daemon_addr() -> DaemonAddr {
    for path in client_socket_candidates() {
        if std::os::unix::net::UnixStream::connect(&path).is_ok() {
            return DaemonAddr::Unix { path };
        }
    }

    if let Ok(url) = std::env::var("VITALS_URL") {
        if !url.is_empty() {
            return DaemonAddr::Tcp { url };
        }
    }

    DaemonAddr::Tcp {
        url: "http://127.0.0.1:8080".to_string(),
    }
}
