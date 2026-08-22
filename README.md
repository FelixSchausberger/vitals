# Vitals

[![CI Pipeline](https://github.com/schausberger/vitals/actions/workflows/ci.yml/badge.svg)](https://github.com/schausberger/vitals/actions)

> Lightweight system health monitoring suite for Linux with systemd integration

## About

Vitals is a system monitoring suite organized as a Cargo workspace with four
crates plus a shared algorithm library:

1. **vitals-core** (`core/`) — Shared data models and API types
2. **scorer** (`scorer/`) — Temporal Weighted Health Score algorithm
3. **vitals-daemon** (`daemon/`) — Backend service with HTTP/JSON API
4. **vitals-tui** (`tui/`) — Terminal UI that consumes the daemon API
5. **vitals-cli** (`cli/`) — Command-line query tool

## Screenshots

| Healthy | Degraded | Critical |
|---------|----------|----------|
| ![Healthy system](.github/assets/tui-healthy.png) | ![Degraded system](.github/assets/tui-degraded.png) | ![Critical system](.github/assets/tui-critical.png) |

## Architecture

```
┌──────────────────────┐         ┌──────────────────────┐
│     vitals-tui       │         │     vitals-cli       │
│                      │         │                      │
│  - Ratatui TUI       │         │  - Status / Issues / │
│  - HTTP client       │         │    History commands  │
│  - Auto-refresh      │         │  - Human/JSON/Ironbar│
└──────────┬───────────┘         └──────────┬───────────┘
           │                                │
           │      HTTP/JSON API             │
           └────────────┬───────────────────┘
                        │
                        ↓
        ┌───────────────────────────────┐
        │       vitals-daemon           │
        │                               │
        │  Endpoints:      Data Sources:│
        │  - /health       - journald   │
        │  - /score        - systemd    │
        │  - /metrics      - procfs     │
        │  - /history                   │
        │                               │
        │  Processing:                  │
        │  - Issue aggregation          │
        │  - Active probes (OOM, etc.)  │
        │  - Threshold notifications    │
        └───────────────┬───────────────┘
                        │
               depends on┼───────┐
                        ↓       ↓
        ┌──────────────┐  ┌──────────────┐
        │ vitals-core  │  │   scorer     │
        │              │  │              │
        │ Issue        │  │ TWHS algo    │
        │ Severity     │  │ Temporal     │
        │ Health*      │  │ frecency     │
        │ API types    │  │ Baseline     │
        └──────────────┘  └──────────────┘
```

### Data Sources

| Source     | What it collects                                |
|------------|-------------------------------------------------|
| journald   | System logs, error/warning aggregation          |
| systemd    | Unit status, restart storms, boot anomalies     |
| procfs     | CPU, memory, disk usage, load average           |

### Active Probes

| Probe           | Detection method                                    |
|-----------------|-----------------------------------------------------|
| OOM kill        | Scan journal entries for kernel OOM kill messages   |
| Restart storm   | Scan journal for systemd start-limit-hit entries    |
| Boot anomaly    | `systemd-analyze blame` at startup; flag slow units |
| RAM trend       | Track per-unit RSS over time; flag monotonic growth |

## Usage

### Nix (Recommended)

```bash
# Run directly
nix run github:schausberger/vitals

# Install to profile
nix profile install github:schausberger/vitals

# Development shell
nix develop
```

### Build from Source

```bash
cargo build --release
```

### Start Daemon

```bash
vitals-daemon                    # HTTP server on :8080
vitals-daemon --once             # One-shot health check
vitals-daemon --once --explain   # Show score breakdown
```

### Run TUI

```bash
vitals-tui --daemon-url http://localhost:8080
# Press 'q' to quit
```

### CLI

```bash
vitals status                    # Human-readable health status
vitals status --detail           # Detailed view with resource bars
vitals status --format json      # JSON output
vitals issues                    # List active issues
vitals history                   # Rolling 7-day score history
```

## API

### Endpoints

| Endpoint    | Method | Description                           |
|-------------|--------|---------------------------------------|
| `/`         | GET    | API info and available endpoints      |
| `/health`   | GET    | Health score, issues, and resources   |
| `/score`    | GET    | Current score with 1h delta           |
| `/metrics`  | GET    | Prometheus exposition format          |
| `/history`  | GET    | Rolling 7-day score history           |
| `/logs`     | GET    | Aggregated journal entries            |

### Example: `/health`

```bash
curl http://localhost:8080/health | jq
```

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
    }
  ],
  "resources": {
    "cpu_usage": 0.5,
    "cpu_penalty": 0.0,
    "cpu_status": "Healthy",
    "memory_usage": 6.0,
    "memory_penalty": 0.0,
    "memory_status": "Healthy",
    "disk_usage": 0.0,
    "disk_penalty": 0.0,
    "disk_status": "Healthy",
    "load_average": 0.20,
    "load_penalty": 0.0,
    "load_status": "Healthy",
    "resource_impact": 0.0,
    "resource_hog_count": 0,
    "top_consumers": []
  }
}
```

## TUI Display Format

```
┌─Health Score──────────────────────────────────────────┐
│ Health Score: 31.3/100 (poor)                         │
└───────────────────────────────────────────────────────┘
┌─Issues────────────────────────────────────────────────┐
│   5× [ERR] kernel: misc dxg error                     │
│   3× [ERR] Failed to start gdrive-mount.service       │
│   2× [WARN] NetworkManager: DNS configuration issue   │
└───────────────────────────────────────────────────────┘
CPU 0.5%  |  MEM 6.0%  |  DISK 0.0%  |  LOAD 0.20
```

## Health Score Algorithm

Vitals uses the **Temporal Weighted Health Score (TWHS)** algorithm:

```
score = 100 × exp(-T / κ)

where T = Σ [w(sev) × frecency × (1 - cascade)] + Σ [α_j × R_j(u_j)]
```

- **Time decay**: Recent issues weigh more than old ones (configurable half-life, default 6h)
- **Cascade attribution**: If `NetworkManager` fails and 9 dependents fail, the 9 downstream failures are attributed to `NetworkManager`
- **Baseline-relative resources**: A gaming desktop at 85% CPU scores differently than a database server at 85%

**Thresholds**: 90-100 excellent · 75-89 good · 50-74 fair · 25-49 poor · 0-24 critical

See `scorer/` for the full algorithm implementation.

## Configuration

Daemon config: `~/.config/vitals/config.toml`

```toml
[daemon]
host = "127.0.0.1"
port = 8080
calculation_interval = 30

[twhs]
decay_half_life_hours = 6.0
sensitivity = 100.0

[twhs.resources]
r_max = 20.0
steepness = 0.5

[notifier]
alert_below = 75.0
cooldown_secs = 1800
```

TUI options:

```bash
vitals-tui --daemon-url http://localhost:8080 --refresh 5
```

## Development

```bash
# Enter dev shell (provides all tools and aliases)
nix develop

# Aliases available in dev shell:
test            # Run all workspace tests
test:daemon     # Test daemon only
test:tui        # Test TUI only
format          # Format all code
lint            # Run clippy
run:daemon      # Start daemon
run:tui         # Start TUI (connects to daemon)
watch:daemon    # Watch and auto-restart daemon
watch:tui       # Watch and auto-restart TUI
build           # Nix build
build:daemon    # Nix build daemon only
build:tui       # Nix build TUI only
```
