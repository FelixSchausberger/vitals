# vitals-tui

Terminal UI for vitals system monitoring.

## Architecture

The vitals monitoring suite consists of three components:

### vitals-core

Shared data structures and models:
- Issue models (severity, trends, metadata)
- Health scoring configuration
- API response types (HealthResponse, LogsResponse, etc.)
- Resource metrics

### vitals-daemon

Backend daemon providing JSON API:
- `/health` - Returns health score, issues, and resource metrics
- `/logs` - Returns aggregated log entries
- Collects data from systemd, journald, and procfs
- Calculates health scores with EWMA smoothing
- Aggregates repeated log entries

### vitals-tui

Terminal UI that consumes the daemon API:
- Displays health score with color coding (green/yellow/red)
- Shows aggregated issues with frequency counts
- Displays system metrics (CPU, memory, disk, load)
- Refreshes automatically
- Keyboard navigation (q to quit)

## Usage

Start the daemon:

```bash
vitals-daemon
# Daemon listens on http://localhost:8080
```

Run the TUI:

```bash
vitals-tui --daemon-url http://localhost:8080
# Press 'q' to quit
```

## Display Format

```
┌─Health Score──────────────────────────────────────┐
│ Health Score: 75.3/100 (good)                     │
└────────────────────────────────────────────────────┘
┌─Issues────────────────────────────────────────────┐
│  5× [ERR]  kernel: misc dxg error                 │
│  3× [ERR]  Failed to start gdrive-mount.service   │
│  2× [WARN] NetworkManager: DNS config issue       │
└────────────────────────────────────────────────────┘
CPU 0.5%  |  MEM 6.0%  |  DISK 0.0%  |  LOAD 0.20
```

## Design Principles

- **Unix-aligned**: Single responsibility, composable
- **TTY-aware**: Uses ANSI colors only in TTY mode
- **Progressive detail**: Summary first, drill down later
- **Aggregation**: Repeated logs collapsed with frequency counts
- **Clean output**: Minimal clutter, high signal-to-noise

## Configuration

TUI configuration:

```bash
# Custom daemon URL
vitals-tui --daemon-url http://10.0.0.5:8080

# Custom refresh interval
vitals-tui --refresh 5
```

## Development

Build:

```bash
cargo build --release
```

Run locally:

```bash
cargo run -- --daemon-url http://localhost:8080
```

## Dependencies

- ratatui - Terminal UI framework
- crossterm - Terminal control
- reqwest - HTTP client (with rustls for TLS)
- tokio - Async runtime
- vitals-core - Shared models

## Future Features

- Log filtering (by service, severity, time)
- Interactive drill-down into full logs
- Help overlay (press `?`)
- Search functionality
- Multiple view modes (summary, detailed, full logs)
