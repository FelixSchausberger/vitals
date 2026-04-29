# Systemd Service Configuration

This directory contains systemd service files for the Vitals monitoring daemon.

## Files

- `vitals-daemon.service` - System-wide daemon service
- `vitals-daemon-user.service` - User-specific daemon service

## Quick Installation

The easiest way to install the daemon is using the provided installation script:

```bash
# Build the daemon first (from workspace root)
cargo build --release --bin vitals-daemon

# Install using the provided script (requires sudo)
sudo ./daemon/scripts/install-daemon.sh
```

The installation script will automatically:
- Create the `vitals` system user and group
- Create required directories (`/etc/vitals`, `/var/log/vitals`, `/var/lib/vitals`)
- Install the daemon binary to `/usr/local/bin/vitals-daemon`
- Install and enable the systemd service
- Create a default configuration file
- Start the service and test the API

## Manual Installation

### System-wide Installation (Recommended)

For system-wide monitoring with full privileges:

```bash
# Build the daemon (from workspace root)
cargo build --release --bin vitals-daemon

# Create system user and group
sudo useradd --system --gid vitals --shell /bin/false \
             --home-dir /var/lib/vitals --create-home \
             --comment "Vitals monitoring daemon" vitals
sudo groupadd --system vitals

# Create directories
sudo mkdir -p /etc/vitals /var/log/vitals /var/lib/vitals
sudo chown -R vitals:vitals /etc/vitals /var/log/vitals /var/lib/vitals
sudo chmod 755 /etc/vitals /var/log/vitals /var/lib/vitals

# Install binary
sudo cp target/release/vitals-daemon /usr/local/bin/vitals-daemon
sudo chmod 755 /usr/local/bin/vitals-daemon
sudo chown root:root /usr/local/bin/vitals-daemon

# Install service
sudo cp daemon/systemd/vitals-daemon.service /etc/systemd/system/
sudo systemctl daemon-reload
sudo systemctl enable vitals-daemon
sudo systemctl start vitals-daemon
```

### User Installation

For user-specific monitoring (limited permissions):

```bash
# Build the daemon (from workspace root)
cargo build --release --bin vitals-daemon

# Create user directories
mkdir -p ~/.config/vitals ~/.local/share/vitals ~/.local/bin

# Install binary to user bin
cp target/release/vitals-daemon ~/.local/bin/vitals-daemon

# Install user service
mkdir -p ~/.config/systemd/user
cp daemon/systemd/vitals-daemon-user.service ~/.config/systemd/user/vitals-daemon.service
systemctl --user daemon-reload
systemctl --user enable vitals-daemon
systemctl --user start vitals-daemon
```

## Service Management

### System Service

```bash
# Status and logs
sudo systemctl status vitals-daemon
sudo journalctl -u vitals-daemon -f

# Control service
sudo systemctl start vitals-daemon
sudo systemctl stop vitals-daemon
sudo systemctl restart vitals-daemon

# Enable/disable autostart
sudo systemctl enable vitals-daemon
sudo systemctl disable vitals-daemon
```

### User Service

```bash
# Status and logs
systemctl --user status vitals-daemon
journalctl --user -u vitals-daemon -f

# Control service
systemctl --user start vitals-daemon
systemctl --user stop vitals-daemon
systemctl --user restart vitals-daemon

# Enable/disable autostart
systemctl --user enable vitals-daemon
systemctl --user disable vitals-daemon
```

## Configuration

Default configuration locations:
- System: `/etc/vitals/daemon.toml`
- User: `~/.config/vitals/daemon.toml`

Example configuration:

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

## API Endpoints

The daemon provides HTTP API endpoints:

- `http://127.0.0.1:8080/` - API information
- `http://127.0.0.1:8080/health` - JSON health data
- `http://127.0.0.1:8080/logs` - Aggregated log entries
- `http://127.0.0.1:8080/metrics` - Prometheus metrics

Test the API:

```bash
# Check daemon status
curl http://127.0.0.1:8080/health | jq

# Get Prometheus metrics
curl http://127.0.0.1:8080/metrics
```

## Troubleshooting

### Common Issues

1. **Service won't start**:
   ```bash
   sudo systemctl status vitals-daemon
   sudo journalctl -u vitals-daemon -n 50
   ```

2. **Binary not found**:
   ```bash
   ls -la /usr/local/bin/vitals-daemon
   # If missing, rebuild and reinstall
   cargo build --release --bin vitals-daemon
   sudo cp target/release/vitals-daemon /usr/local/bin/vitals-daemon
   ```

3. **User/group missing**:
   ```bash
   id vitals
   # If missing, create user/group
   sudo useradd --system --group vitals
   ```

4. **Permission issues**:
   ```bash
   sudo chown -R vitals:vitals /etc/vitals /var/log/vitals /var/lib/vitals
   sudo chmod 755 /etc/vitals /var/log/vitals /var/lib/vitals
   ```

5. **API not responding**:
   ```bash
   # Check if port is listening
   sudo ss -tlnp | grep :8080

   # Check if service is active
   sudo systemctl is-active vitals-daemon
   ```

### Logs Analysis

Check logs for specific issues:

```bash
# Service startup issues
sudo journalctl -u vitals-daemon --since "5 minutes ago"

# Configuration issues
sudo journalctl -u vitals-daemon | grep -i "config\|error"

# API binding issues
sudo journalctl -u vitals-daemon | grep -i "bind\|listen\|port"
```

## Security Features

The systemd service includes security hardening:

- **Dedicated user**: Runs as `vitals` system user
- **No shell access**: User has `/bin/false` shell
- **Protected directories**: Limited filesystem access
- **No new privileges**: `NoNewPrivileges=yes`
- **System call filtering**: Restricted to essential calls
- **Private temp**: `PrivateTmp=yes`
- **Resource limits**: Memory and task limits

## Integration with TUI

The TUI application connects to the daemon via HTTP:

```bash
# Start daemon (in one terminal)
cargo run --release --bin vitals-daemon

# Start TUI (in another terminal)
cargo run --release --bin vitals-tui -- --daemon-url http://localhost:8080
```

## Nix Installation

If using Nix flakes:

```bash
# Build daemon with Nix
nix build .#daemon

# Install systemd service
nix build .#systemd-service
sudo cp result/lib/systemd/system/vitals-daemon.service /etc/systemd/system/
sudo systemctl daemon-reload
sudo systemctl enable --now vitals-daemon
```
