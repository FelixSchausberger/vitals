# Vitals Roadmap

## v0.1 — Release Blockers

All blockers resolved. See git history for details.

---

## v0.1+ — Post-Release

Can ship after v0.1. Ordered by impact.

### 1. TUI: log filtering ✅

Filter by service (`u`), severity (`a`), time range (`t`); `r` resets.
Server-side filtering via `/logs` query params (`severity`, `unit`, `since`,
`until`, `limit`, `offset`) defined in `vitals_core::api::LogsQuery`.

### 2. TUI: drill-down ✅

Enter expands issue details (with related logs per unit) and full log
entries; j/k scrolls the pane; Esc closes.

### 3. TUI: search ✅

Regex search through logs (`/`, Enter to apply, n/N to navigate). Runs
client-side over the fetched window; revisit server-side pushdown only if
`max_journal_entries` grows by an order of magnitude.

### 4. TUI: help overlay ✅

Press `?` for keyboard shortcuts. Rendering now lives in `tui/src/ui/`;
state and input handling in `tui/src/app.rs`.

### 5. TUI: multiple view modes ✅

Summary, Detailed, Full logs. Tab cycles; 1/2/3 jump directly.

### 6. TUI: config file ✅

`~/.config/vitals/tui.toml`: `daemon_url`, `daemon_socket`, `refresh_secs`,
`default_view`. CLI arguments override file values.

### 7. Performance: caching & pagination ✅

Server-side pagination (`limit`/`offset`, page size 200, `,`/`.` or
PgUp/PgDn) plus refetch suppression when cached logs are fresher than one
refresh interval. Incremental append was deliberately dropped: the daemon's
server-side aggregation would need to be replicated client-side to merge
correctly.

### 8. Optional systemd/zbus feature flag ✅

`vitals-daemon` builds without systemd deps via `--no-default-features`
(`systemd` feature is default). Live mode errors clearly when disabled;
mock mode works. CI checks both configurations.

### 9. Consolidate sysinfo vs procfs ✅

Standardized on procfs (Linux-only target). System-wide CPU/memory/load and
per-process/unit metrics all read from `/proc`; sysinfo dependency removed.
Disk usage is now measured via statfs over `/proc/mounts` (was a 0.0
placeholder under sysinfo).

### 10. Publish to crates.io ✅ (prep complete)

Path deps carry version constraints, readme/metadata audited, name
availability confirmed, ordered publish workflow added
(`.github/workflows/release.yml`, manual trigger, dry-run default).
Actual upload requires the `CARGO_REGISTRY_TOKEN` secret.

(End of file - total 52 lines)
