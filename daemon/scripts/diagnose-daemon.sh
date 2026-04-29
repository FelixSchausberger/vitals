#!/bin/bash
# Diagnose vitals-daemon startup issues

echo "🔍 Vitals Daemon Diagnostics"
echo "=============================="

# Check if daemon binary exists and is executable
echo
echo "📁 Binary Check:"
if command -v vitals-daemon &> /dev/null; then
    echo "✅ vitals-daemon found in PATH: $(which vitals-daemon)"
else
    echo "❌ vitals-daemon not found in PATH"
    echo "   Try: cargo build --release --bin vitals-daemon"
fi

if [ -f "/usr/local/bin/vitals-daemon" ]; then
    echo "✅ /usr/local/bin/vitals-daemon exists"
else
    echo "❌ /usr/local/bin/vitals-daemon not found (required for system service)"
fi

# Check port availability
echo
echo "🌐 Port Check:"
if command -v ss &> /dev/null; then
    if ss -tulpn | grep -q ":8080 "; then
        echo "❌ Port 8080 is already in use:"
        ss -tulpn | grep ":8080 " | head -3
        echo "   Consider changing port in config or stopping conflicting service"
    else
        echo "✅ Port 8080 is available"
    fi
elif command -v netstat &> /dev/null; then
    if netstat -tulpn | grep -q ":8080 "; then
        echo "❌ Port 8080 is already in use:"
        netstat -tulpn | grep ":8080 " | head -3
    else
        echo "✅ Port 8080 is available"
    fi
else
    echo "⚠️  Cannot check port status (ss/netstat not available)"
fi

# Check system user for system service
echo
echo "👤 System User Check:"
if id -u vitals &> /dev/null; then
    echo "✅ vitals user exists"
else
    echo "❌ vitals user not found (required for system service)"
    echo "   Try: sudo useradd -r -s /bin/false vitals"
fi

if getent group vitals &> /dev/null; then
    echo "✅ vitals group exists"
else
    echo "❌ vitals group not found (required for system service)"
    echo "   Try: sudo groupadd -r vitals"
fi

# Check required directories
echo
echo "📂 Directory Check:"
for dir in "/etc/vitals" "/var/log/vitals" "/var/lib/vitals"; do
    if [ -d "$dir" ]; then
        echo "✅ $dir exists"
        # Check ownership and permissions
        if [ -O "$dir" ] || [ "$(stat -c %U "$dir")" = "vitals" ]; then
            echo "   Owned by appropriate user"
        else
            echo "   ⚠️  May need ownership change: sudo chown -R vitals:vitals $dir"
        fi
    else
        echo "❌ $dir missing (required for system service)"
        echo "   Try: sudo mkdir -p $dir && sudo chown vitals:vitals $dir"
    fi
done

# Check systemd service status
echo
echo "⚙️  Service Status:"
if systemctl --user list-unit-files | grep -q vitals-daemon; then
    echo "✅ User service installed"
    systemctl --user status vitals-daemon --no-pager -l | head -10
else
    echo "ℹ️  User service not installed"
fi

if systemctl list-unit-files | grep -q vitals-daemon; then
    echo "✅ System service installed"
    sudo systemctl status vitals-daemon --no-pager -l | head -10
else
    echo "ℹ️  System service not installed"
fi

# Check recent journal logs
echo
echo "📜 Recent Logs:"
if journalctl --user -u vitals-daemon --no-pager -n 5 &> /dev/null; then
    echo "User service logs:"
    journalctl --user -u vitals-daemon --no-pager -n 5
fi

if sudo journalctl -u vitals-daemon --no-pager -n 5 &> /dev/null; then
    echo "System service logs:"
    sudo journalctl -u vitals-daemon --no-pager -n 5
fi

# Test daemon manually
echo
echo "🧪 Manual Test:"
echo "Try running the daemon manually:"
echo "   cargo run --release --bin vitals-daemon"
echo "   curl http://localhost:8080/health"

# Check workspace build
echo
echo "🏗️  Workspace Build Check:"
script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
daemon_dir="$(dirname "$script_dir")"
project_root="$(dirname "$daemon_dir")"

if [ -f "$project_root/target/release/vitals-daemon" ]; then
    echo "✅ Daemon binary found in workspace: $project_root/target/release/vitals-daemon"
    ls -lh "$project_root/target/release/vitals-daemon"
else
    echo "⚠️  Daemon binary not found in workspace"
    echo "   Build with: cd $project_root && cargo build --release --bin vitals-daemon"
fi

echo
echo "✅ Diagnosis complete!"
