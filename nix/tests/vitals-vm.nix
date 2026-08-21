# NixOS VM Integration Test: Vitals Daemon
#
# Validates end-to-end through the real NixOS module (nix/vitals.nix):
# the daemon starts as a hardened systemd system service and serves
# the HTTP API over TCP and its Unix socket.
# Uses mock mode to avoid requiring real systemd journal data.
{ inputs, ... }:
let
  vitalsModule = import ../vitals.nix inputs;
in
{
  name = "vitals-vm";

  nodes.machine =
    { pkgs, ... }:
    {
      imports = [ vitalsModule ];
      services.vitals.enable = true;
      services.vitals.mode = "mock";

      environment.systemPackages = [
        pkgs.curl
        pkgs.jq
      ];
    };

  testScript = ''
    machine.wait_for_unit("multi-user.target")
    machine.wait_for_unit("vitals-daemon.service")

    # Wait for the daemon's background TCP listener (default port 8080)
    machine.wait_for_open_port(8080)

    # Wait until the first health calculation tick has produced data
    machine.wait_until_succeeds("curl -sf http://127.0.0.1:8080/score | jq -e '.score >= 0'")

    # 1. Root endpoint lists /health and /logs
    root = machine.succeed("curl -s http://127.0.0.1:8080/")
    assert '"/health"' in root, f"Root endpoint missing /health: {root}"
    assert '"/logs"' in root, f"Root endpoint missing /logs: {root}"

    # 2. /health returns valid health data
    health = machine.succeed("curl -s http://127.0.0.1:8080/health | jq -r '.status'")
    assert health.strip() in ("excellent", "good", "fair", "poor", "critical"), \
      f"Unexpected health status: {health}"

    # 3. /score returns numeric score in range
    score = machine.succeed("curl -s http://127.0.0.1:8080/score | jq -r '.score'")
    score_val = float(score.strip())
    assert 0.0 <= score_val <= 100.0, f"Score out of range: {score_val}"

    # 4. /logs returns entry list
    logs = machine.succeed("curl -s http://127.0.0.1:8080/logs | jq -r '.total'")
    total = int(logs.strip())
    assert total >= 0, f"Log total is negative: {total}"

    # 5. /metrics returns Prometheus format
    metrics = machine.succeed("curl -s http://127.0.0.1:8080/metrics")
    assert "vitals_health_score" in metrics, f"Metrics missing expected metric: {metrics[:200]}"

    # 6. Primary transport: Unix socket serves the same API.
    #    The daemon appends "vitals/daemon.sock" to $XDG_RUNTIME_DIR.
    sock_health = machine.succeed(
      "curl -s --unix-socket /run/vitals/vitals/daemon.sock http://localhost/health | jq -r '.status'"
    )
    assert sock_health.strip() == health.strip(), \
      f"Unix socket health mismatch: tcp={health} unix={sock_health}"

    print("All vitals daemon endpoints verified successfully")
  '';
}
