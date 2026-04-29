# Vitals

> Lightweight system health monitoring suite for Linux with systemd integration

**Status**: Cargo workspace monorepo with 3-crate architecture

## Overview

Vitals is a system monitoring suite organized as a Cargo workspace with three cleanly separated components:

1. **vitals-core** (`core/`) - Shared data models and API types
2. **vitals-daemon** (`daemon/`) - Backend daemon with JSON API
3. **vitals-tui** (`tui/`) - Terminal UI that consumes the API

## Architecture Diagram

```
┌──────────────────┐
│ vitals-tui  │  Terminal UI
│                  │
│  - Ratatui TUI   │  Displays:
│  - HTTP client   │  - Health score (colored)
│  - Auto-refresh  │  - Aggregated issues
│                  │  - System metrics
└────────┬─────────┘
         │
         │ HTTP/JSON API
         │
         ↓
┌────────────────────┐
│ vitals-daemon │  Backend Service
│                    │
│  Endpoints:        │  Data Sources:
│  - /health         │  - journald (logs)
│  - /logs           │  - systemd (units)
│  - /metrics        │  - procfs (metrics)
│                    │
│  Processing:       │
│  - Issue aggreg.   │
│  - Health scoring  │
│  - EWMA smoothing  │
└────────┬───────────┘
         │
         │ depends on
         ↓
┌──────────────────┐
│ vitals-core │  Shared Library
│                  │
│  - Issue         │  Data models
│  - Severity      │  for both daemon
│  - Health*       │  and TUI
│  - API types     │
└──────────────────┘
```

## Component Details

### vitals-core (`core/`)

**Purpose**: Shared data structures for serialization and API contracts

**Key Types**:
- `Issue`, `Severity`, `IssueTrend` - Issue models
- `HealthBreakdown`, `HealthConfig` - Health scoring structures
- `ResourceHealthMetrics`, `ResourceStatus` - Resource monitoring
- `HealthResponse`, `LogsResponse` - API response types
- `LogEntry`, `MetricsSummary` - API data structures

**Dependencies**:
- serde (serialization)
- time (timestamps)
- thiserror (errors)

**Status**: ✅ Built and working

### vitals-daemon (`daemon/`)

**Purpose**: Backend daemon that monitors system health and provides HTTP API

**Key Modules**:
- `health` - HealthCalculator with EWMA smoothing
- `agg` - Issue aggregation logic
- `data` - Data collectors (journald, systemd, procfs)
- `config` - Configuration management
- `bin/daemon` - HTTP server (Axum)

**API Endpoints**:
- `GET /health` - Health score + issues + resources (JSON)
- `GET /logs` - Aggregated log entries (JSON)
- `GET /metrics` - Prometheus exposition format
- `GET /` - API info

**Dependencies**:
- vitals-core (models)
- tokio (async runtime)
- axum (HTTP server)
- zbus (systemd D-Bus)
- systemd (journald)
- sysinfo, procfs (metrics)

**Status**: ⚠️ Partially refactored (needs systemd libs to build)

### vitals-tui (`tui/`)

**Purpose**: Terminal UI that consumes daemon API

**Key Modules**:
- `main` - TUI event loop and rendering
- `client` - DaemonClient for HTTP requests
- `ui` - UI utilities (future expansion)

**Features**:
- Health score display with color coding (green/yellow/red)
- Aggregated issues with frequency counts (5× [ERR] ...)
- System metrics bar (CPU, MEM, DISK, LOAD)
- Auto-refresh (default: 2s)
- Keyboard controls (q to quit)

**Dependencies**:
- vitals-core (models)
- ratatui (TUI framework)
- crossterm (terminal control)
- reqwest (HTTP client with rustls)
- tokio (async runtime)

**Status**: ✅ Built and ready to use

## Design Principles

### 1. Clean Separation

- **Daemon**: Collects data, calculates scores, exposes API
- **TUI**: Consumes API, renders UI
- **Core**: Shared contract between daemon and TUI

### 2. Unix Philosophy

- Do one thing well
- Composable components
- Standard formats (JSON, Prometheus)
- TTY-aware output

### 3. Information Hierarchy

Display order:
1. Health score (most critical)
2. Errors
3. Warnings
4. Info/metrics

### 4. Aggregation Over Noise

- Repeated logs collapsed: `5× [ERR] kernel error`
- Focus on unique issues, not log volume

### 5. Progressive Detail

- Default: one-screen summary
- Future: interactive drill-down

## TUI Display Format

```
┌─Health Score──────────────────────────────────────┐
│ Health Score: 31.3/100 (poor)                     │
└────────────────────────────────────────────────────┘
┌─Issues────────────────────────────────────────────┐
│  5× [ERR]  Oct 07 10:07:06 hp-probook-wsl        │
│      kernel: misc dxg: dxgkio error               │
│                                                    │
│  3× [ERR]  Failed to start Google Drive Mount    │
│      (gdrive-mount.service)                       │
│                                                    │
│  2× [WARN] NetworkManager: DNS configuration     │
│      issue detected                               │
└────────────────────────────────────────────────────┘
CPU 0.5%  |  MEM 6.0%  |  DISK 0.0%  |  LOAD 0.20
```

## API Example

### Request

```bash
curl http://localhost:8080/health | jq
```

### Response

```json
{
  "status": "poor",
  "score": 31.3,
  "raw_score": 28.7,
  "heartbeat": "red",
  "timestamp": 1704067200,
  "breakdown": {
    "errors": 8,
    "warnings": 3,
    "info": 2,
    "total": 13
  },
  "issues": [
    {
      "id": "kernel-dxg-error",
      "title": "kernel: misc dxg error",
      "severity": "Error",
      "count": 5,
      "impact": -23.1
    },
    {
      "id": "gdrive-mount-failed",
      "title": "Failed to start gdrive-mount.service",
      "severity": "Error",
      "count": 3,
      "impact": -10.9
    }
  ],
  "resources": {
    "cpu_usage": 0.5,
    "cpu_status": "Healthy",
    "memory_usage": 6.0,
    "memory_status": "Healthy",
    "disk_usage": 0.0,
    "disk_status": "Healthy",
    "load_average": 0.20,
    "load_status": "Healthy",
    "resource_impact": 0.0,
    "resource_hog_count": 0,
    "top_consumers": []
  }
}
```

## Health Score Algorithm

**Formula**:
```
score = 100 - (issue_penalties + resource_penalties)
smoothed_score = EWMA(score, alpha=0.3)
```

**Issue Penalties**:
- Error: `-10 × ln(count)`
- Warning: `-3 × ln(count)`
- Info: `-1 × ln(count)`

**Resource Penalties**:
- Warning status: `-2 per resource`
- Critical status: `-8 per resource`
- Applied to: CPU, memory, disk, load average

**Thresholds**:
- 90-100: Excellent (green)
- 75-89: Good (green)
- 50-74: Fair (yellow)
- 25-49: Poor (yellow)
- 0-24: Critical (red)

## Usage

### Build All Components

```bash
# From workspace root
cargo build --release
```

### Start Daemon

```bash
# From workspace root
cargo run --release --bin vitals-daemon
# Listens on http://localhost:8080

# Or from daemon directory
cd daemon
cargo run --release
```

### Run TUI

```bash
# From workspace root
cargo run --release --bin vitals-tui -- --daemon-url http://localhost:8080

# Or from tui directory
cd tui
cargo run --release -- --daemon-url http://localhost:8080
# Press 'q' to quit
```

### Query API

```bash
# Health endpoint
curl http://localhost:8080/health | jq '.score'

# Prometheus metrics
curl http://localhost:8080/metrics
```

## Configuration

Daemon config: `~/.config/vitals/daemon.toml`

```toml
[daemon]
host = "127.0.0.1"
port = 8080
calculation_interval = 10

[health]
ewma_window = 10
ewma_alpha = 0.3
error_weight = 10.0
warning_weight = 3.0
info_weight = 1.0

[health.resource_thresholds]
cpu_warning_threshold = 80.0
memory_warning_threshold = 85.0
enable_resource_monitoring = true
```

TUI options:

```bash
vitals-tui --daemon-url http://localhost:8080 --refresh 5
```

## Next Steps

### Immediate

1. Fix daemon build (systemd dependencies)
2. Test full integration (daemon + TUI)
3. Add `/logs` endpoint implementation

### Future Features

- **Filtering**: By service, severity, time range
- **Search**: Regex search through logs
- **Drill-down**: Press Enter to expand issue details
- **Help overlay**: Press `?` for keyboard shortcuts
- **Configuration**: TUI config file
- **Performance**: Caching, pagination for large datasets

## Repository Structure

```
vitals/                     # Cargo workspace root
├── Cargo.toml                   # Workspace configuration
├── README.md                    # This file
│
├── core/                        # vitals-core crate
│   ├── Cargo.toml
│   └── src/
│       ├── lib.rs
│       ├── issue.rs
│       ├── health.rs
│       └── api.rs
│
├── daemon/                      # vitals-daemon crate
│   ├── Cargo.toml
│   ├── README.md
│   └── src/
│       ├── lib.rs
│       ├── health.rs
│       ├── agg/
│       ├── data/
│       └── bin/daemon.rs
│
├── tui/                         # vitals-tui crate
│   ├── Cargo.toml
│   ├── README.md
│   └── src/
│       ├── main.rs
│       ├── client.rs
│       └── ui.rs
│
└── backup/                      # Legacy monorepo (archived)
    └── ...
```

## Development

### Workspace Commands

```bash
# Build all crates
cargo build --workspace

# Test all crates
cargo test --workspace

# Run specific binary
cargo run --bin vitals-daemon
cargo run --bin vitals-tui

# Check all crates
cargo check --workspace

# Format code
cargo fmt --all

# Lint with clippy
cargo clippy --workspace --all-targets
```

### Publishing to crates.io

Each crate can be published independently:

```bash
# Publish core first (no dependencies)
cd core && cargo publish

# Then daemon (depends on core)
cd daemon && cargo publish

# Finally tui (depends on core)
cd tui && cargo publish
```

## CI/CD Pipeline

The project uses a hybrid CI/CD approach with Garnix and GitHub Actions.

### Garnix CI (Primary Build System)

Garnix handles heavy Rust compilation with centralized signing for enhanced security:
- Daemon and TUI package builds (x86_64-linux, aarch64-linux)
- Systemd service package builds
- Full workspace integration tests

Configuration: `garnix.yaml`

Setup: Install Garnix GitHub App at https://garnix.io (one-time manual setup)

### GitHub Actions (Validation & Security)

GitHub Actions handles lightweight validation and security scanning:
- Security scans (Trivy vulnerability scanner, cargo audit)
- Pre-commit hooks validation (prek)
- Code quality checks (rustfmt, clippy)
- Workspace structure validation

Configuration: `.github/workflows/ci.yml`

### Binary Caches

Multiple caches configured with priority-based fallback in `flake.nix`:
- cache.nixos.org - Official NixOS cache
- nix-community.cachix.org - Community packages
- cache.garnix.io - Garnix CI builds with centralized signing

Garnix cache uses centralized signing, reducing cache poisoning risks compared to traditional binary caches where multiple contributors have push access. This provides better security for Rust dependency compilation.

## Summary

This Cargo workspace successfully organizes the vitals project:

✅ **Unified repository** - Single git clone, unified CI/CD
✅ **Clean separation** - Three focused crates with clear responsibilities
✅ **Shared dependencies** - Consistent versions across all components
✅ **Independent binaries** - `vitals-daemon` and `vitals-tui`

The architecture follows Rust best practices and Unix principles, providing clean APIs and enabling future expansion.
