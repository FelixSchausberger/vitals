#!/bin/bash
# Vitals Daemon Installation Script
# This script installs the vitals daemon as a systemd service

set -e

# Configuration
DAEMON_NAME="vitals-daemon"
DAEMON_USER="vitals"
DAEMON_GROUP="vitals"
CONFIG_DIR="/etc/vitals"
LOG_DIR="/var/log/vitals"
DATA_DIR="/var/lib/vitals"
SYSTEMD_DIR="/etc/systemd/system"

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[0;33m'
NC='\033[0m' # No Color

# Logging functions
log_info() {
	echo -e "${GREEN}[INFO]${NC} $1"
}

log_warn() {
	echo -e "${YELLOW}[WARN]${NC} $1"
}

log_error() {
	echo -e "${RED}[ERROR]${NC} $1"
}

# Check if running as root
check_root() {
	if [[ $EUID -ne 0 ]]; then
		log_error "This script must be run as root (use sudo)"
		exit 1
	fi
}

# Create system user and group
create_user() {
	log_info "Creating system user and group: $DAEMON_USER"

	if ! getent group "$DAEMON_GROUP" >/dev/null 2>&1; then
		groupadd --system "$DAEMON_GROUP"
		log_info "Created group: $DAEMON_GROUP"
	else
		log_warn "Group $DAEMON_GROUP already exists"
	fi

	if ! getent passwd "$DAEMON_USER" >/dev/null 2>&1; then
		useradd --system --gid "$DAEMON_GROUP" --shell /bin/false \
			--home-dir "$DATA_DIR" --create-home \
			--comment "Vitals monitoring daemon" "$DAEMON_USER"
		log_info "Created user: $DAEMON_USER"
	else
		log_warn "User $DAEMON_USER already exists"
	fi
}

# Create required directories
create_directories() {
	log_info "Creating required directories"

	for dir in "$CONFIG_DIR" "$LOG_DIR" "$DATA_DIR"; do
		if [[ ! -d "$dir" ]]; then
			mkdir -p "$dir"
			log_info "Created directory: $dir"
		else
			log_warn "Directory $dir already exists"
		fi
	done

	# Set ownership
	chown -R "$DAEMON_USER:$DAEMON_GROUP" "$CONFIG_DIR" "$LOG_DIR" "$DATA_DIR"

	# Set permissions
	chmod 755 "$CONFIG_DIR" "$LOG_DIR" "$DATA_DIR"

	log_info "Set ownership and permissions for daemon directories"
}

# Install daemon binary
install_binary() {
	local binary_path="$1"
	local target_path="/usr/local/bin/$DAEMON_NAME"

	if [[ ! -f "$binary_path" ]]; then
		log_error "Daemon binary not found at: $binary_path"
		log_error "Please build the daemon first with: cargo build --release --bin vitals-daemon"
		exit 1
	fi

	log_info "Installing daemon binary to $target_path"
	cp "$binary_path" "$target_path"
	chmod 755 "$target_path"
	chown root:root "$target_path"

	log_info "Daemon binary installed successfully"
}

# Install systemd service
install_service() {
	local service_file="$1"
	local target_file="$SYSTEMD_DIR/$DAEMON_NAME.service"

	if [[ ! -f "$service_file" ]]; then
		log_error "Service file not found at: $service_file"
		exit 1
	fi

	log_info "Installing systemd service to $target_file"
	cp "$service_file" "$target_file"
	chmod 644 "$target_file"
	chown root:root "$target_file"

	# Reload systemd to recognize the new service
	systemctl daemon-reload

	log_info "Systemd service installed successfully"
}

# Create default configuration
create_default_config() {
	local config_file="$CONFIG_DIR/daemon.toml"

	if [[ -f "$config_file" ]]; then
		log_warn "Configuration file already exists at $config_file"
		return
	fi

	log_info "Creating default configuration at $config_file"

	cat >"$config_file" <<'EOF'
# Vitals Daemon Configuration

[daemon]
host = "127.0.0.1"
port = 8080
calculation_interval = 10

[twhs]
error_weight = 10.0
warning_weight = 3.162
info_weight = 1.0

[health.resource_thresholds]
cpu_warning_threshold = 80.0
memory_warning_threshold = 85.0
enable_resource_monitoring = true
EOF

	chown "$DAEMON_USER:$DAEMON_GROUP" "$config_file"
	chmod 644 "$config_file"

	log_info "Default configuration created"
}

# Enable and start service
enable_service() {
	log_info "Enabling and starting $DAEMON_NAME service"

	systemctl enable "$DAEMON_NAME"
	systemctl start "$DAEMON_NAME"

	# Wait a moment for the service to start
	sleep 2

	if systemctl is-active --quiet "$DAEMON_NAME"; then
		log_info "Service $DAEMON_NAME started successfully"
		systemctl status "$DAEMON_NAME" --no-pager --lines=0
	else
		log_error "Service $DAEMON_NAME failed to start"
		log_error "Check logs with: journalctl -u $DAEMON_NAME -f"
		exit 1
	fi
}

# Test daemon API
test_daemon() {
	log_info "Testing daemon API endpoints"

	local daemon_url="http://127.0.0.1:8080"

	if command -v curl >/dev/null 2>&1; then
		# Test health endpoint
		if curl -s "$daemon_url/health" >/dev/null; then
			log_info "Daemon API is responding on $daemon_url"
			log_info "Health endpoint: $daemon_url/health"
			log_info "Metrics endpoint: $daemon_url/metrics"
			log_info "Logs endpoint: $daemon_url/logs"
		else
			log_warn "Daemon API is not responding yet (this is normal during first startup)"
		fi
	else
		log_warn "curl not available, skipping API test"
	fi
}

# Main installation function
main() {
	log_info "Starting Vitals daemon installation"

	# Determine paths - navigate from daemon/scripts/ to workspace root
	local script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
	local daemon_dir="$(dirname "$script_dir")"
	local project_root="$(dirname "$daemon_dir")"
	local binary_path="$project_root/target/release/vitals-daemon"
	local service_file="$daemon_dir/systemd/vitals-daemon.service"

	# Check prerequisites
	check_root

	# Stop service if it's already running
	if systemctl is-active --quiet "$DAEMON_NAME" 2>/dev/null; then
		log_info "Stopping existing $DAEMON_NAME service"
		systemctl stop "$DAEMON_NAME"
	fi

	# Installation steps
	create_user
	create_directories
	install_binary "$binary_path"
	install_service "$service_file"
	create_default_config
	enable_service
	test_daemon

	log_info "Vitals daemon installation completed successfully!"
	log_info ""
	log_info "Useful commands:"
	log_info "  Status:  systemctl status $DAEMON_NAME"
	log_info "  Logs:    journalctl -u $DAEMON_NAME -f"
	log_info "  Stop:    systemctl stop $DAEMON_NAME"
	log_info "  Start:   systemctl start $DAEMON_NAME"
	log_info "  Restart: systemctl restart $DAEMON_NAME"
	log_info "  Health:  curl http://127.0.0.1:8080/health"
	log_info "  Metrics: curl http://127.0.0.1:8080/metrics"
}

# Run main function
main "$@"
