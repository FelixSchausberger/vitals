# Vitals Daemon

> Lightweight system health monitoring daemon for Linux with systemd integration

Vitals daemon monitors your system's health by analyzing journald logs and systemd units, calculating a health score based on errors, warnings, and resource utilization.

## Features

- **Health Scoring**: Calculates 0-100 health score based on:
  - System errors and warnings from journald
  - Failed systemd units
  - Resource utilization (CPU, memory, disk, load)
  - Resource-intensive processes

- **Multiple Output Formats**:
  - Human-readable text
  - JSON (for scripting and integration)
  - Prometheus exposition format (for monitoring)

- **Two Modes**:
  - **Daemon mode**: HTTP server exposing `/health` and `/metrics` endpoints
  - **One-shot mode**: Single health check for scripts and CLI usage

## Installation

### Using Nix (recommended)

```bash
# Clone repository
git clone https://github.com/schausberger/vitals-daemon
cd vitals-daemon

# Enter dev shell
nix develop

# Build
cargo build --release

# Or build with Nix
nix build
```

### Using Cargo

```bash
cargo install --path .
```

## Usage

### One-Shot Mode

```bash
# Simple health check
vitals-daemon --once
# Output: Health: 87.3/100 (good) [green]

# JSON output (for scripting)
vitals-daemon --once --format json | jq '.score'

# Prometheus format
vitals-daemon --once --format prometheus

# Detailed explanation
vitals-daemon --once --explain
```

Example `--explain` output:
```
Health Score: 73.2/100 (good)
Raw Score: 71.5/100

Score Breakdown:
  Base: 100.0

  Issue Impacts:
    [Error] systemd-resolved failed (5 occurrences) → -15.3
    [Error] docker.service timeout (2 occurrences) → -7.8
    [Warning] High CPU usage alert (1 occurrence) → -2.1
    Total issue impact: -25.2

  Resource Impacts:
    CPU: 45.2% (Healthy)
    Memory: 68.3% (Healthy)
    Disk: 71.5% (Healthy)
    Load: 1.23 (Healthy)
    Total resource impact: -0.0

  Top Resource Consumers:
    docker.service (CPU: 35.2%, Memory: 1024MB, Impact: 8.6)
    chrome.service (CPU: 15.8%, Memory: 512MB, Impact: 4.1)

Summary:
  2 error(s)
  1 warning(s)
  0 info message(s)
```

### Daemon Mode

```bash
# Start HTTP server (default: localhost:8080)
vitals-daemon

# Custom port
vitals-daemon --config config.toml

# With debug logging
vitals-daemon --debug
```

**HTTP Endpoints:**

- `GET /health` - JSON health breakdown
- `GET /metrics` - Prometheus metrics
- `GET /` - Service info

Example `/health` response:
```json
{
  "status": "good",
  "score": 87.3,
  "raw_score": 85.1,
  "heartbeat": "green",
  "timestamp": 1735689600,
  "breakdown": {
    "errors": 1,
    "warnings": 2,
    "info": 0,
    "total": 3
  },
  "issues": [
    {
      "id": "systemd-unit-failed-dockerd",
      "title": "docker.service failed",
      "severity": "Error",
      "count": 1,
      "impact": -8.5
    }
  ],
  "resources": {
    "cpu_usage": 45.2,
    "memory_usage": 68.3,
    "disk_usage": 71.5,
    "load_average": 1.23
  }
}
```

## Integration Examples

### Waybar

```json
{
  "custom/health": {
    "exec": "vitals-daemon --once --format json",
    "return-type": "json",
    "format": "{} {icon}",
    "format-icons": {
      "green": "💚",
      "yellow": "💛",
      "red": "❤️"
    },
    "interval": 30
  }
}
```

### Ironbar

```toml
[[module]]
type = "custom"
name = "health"
exec = "vitals-daemon --once | awk '{print $2}'"
interval = 30
```

### Shell Scripts

```bash
#!/bin/bash
# Check if system health is critical before running updates
SCORE=$(vitals-daemon --once --format json | jq '.score')
if (( $(echo "$SCORE < 50" | bc -l) )); then
  echo "System health critical ($SCORE), skipping updates"
  exit 1
fi
```

### Prometheus

```yaml
scrape_configs:
  - job_name: 'vitals'
    static_configs:
      - targets: ['localhost:8080']
    metrics_path: '/metrics'
```

## Configuration

Create `~/.config/vitals/config.toml`:

```toml
[health]
ewma_alpha = 0.3
error_weight = 10.0
warning_weight = 3.0
info_weight = 1.0

[health.resource_thresholds]
enable_resource_monitoring = true
cpu_warning_threshold = 80.0
cpu_error_threshold = 95.0
memory_warning_threshold = 85.0
memory_error_threshold = 95.0

[daemon]
host = "127.0.0.1"
port = 8080
calculation_interval = 30
max_journal_entries = 1000
journal_time_window_hours = 24

[aggregation]
deduplicate_by_message = true
group_by_unit = true
```

## Health Score Calculation

The health score starts at 100 (perfect health) and deducts points for:

1. **Issues**:
   - Errors: -10 points × ln(count)
   - Warnings: -3 points × ln(count)
   - Info: -1 point × ln(count)

2. **Resources** (if enabled):
   - Critical resource usage: -8 points per resource
   - Warning resource usage: -2 points per resource

3. **EWMA Smoothing**: Smooths score over time to avoid spikes

**Status Levels**:
- 90-100: Excellent (green)
- 75-89: Good (green)
- 50-74: Fair (yellow)
- 25-49: Poor (yellow)
- 0-24: Critical (red)

## Requirements

- Linux with systemd
- journald for log collection
- Rust 1.89+ (for building)

## Development

```bash
# Enter nix shell
nix develop

# Run with mock data
cargo run -- --mode mock --once --explain

# Run tests
cargo nextest run

# Watch and auto-rebuild
cargo watch -x 'run -- --once'
```

## Repository Structure

The vitals project is organized as three separate components:

```
vitals/
├── vitals-core/       # Shared data models and API types
│   ├── src/
│   │   ├── lib.rs
│   │   ├── issue.rs
│   │   ├── health.rs
│   │   └── api.rs
│   └── Cargo.toml
│
├── vitals-daemon/     # Backend daemon with JSON API
│   ├── src/
│   │   ├── agg/            # Issue aggregation
│   │   ├── config/         # Configuration management
│   │   ├── data/           # systemd/journald readers
│   │   ├── bin/daemon.rs   # HTTP server binary
│   │   ├── health.rs       # Health calculation
│   │   └── lib.rs          # Library root
│   ├── Cargo.toml
│   ├── flake.nix           # Nix build
│   └── README.md           # This file
│
└── vitals-tui/        # Terminal UI
    ├── src/
    │   ├── main.rs
    │   ├── client.rs
    │   └── ui.rs
    └── Cargo.toml
```

## License

MIT

## Contributing

PRs welcome! Please keep changes focused and maintain the Unix philosophy.
