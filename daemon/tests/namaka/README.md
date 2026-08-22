# Namaka Tests

Snapshot tests for the daemon flake using [namaka](https://github.com/nix-community/namaka).

The tests are wired into the flake via `checks = namaka.lib.load { src = ./tests/namaka; }`
in `flake.nix`, so they run as part of `nix flake check` in this directory.

## Layout

- `<name>/expr.nix` - the expression under test
- `_snapshots/<name>` - expected serialized value (`#json\n<value>`)

## Running Tests

```bash
# Install namaka
nix profile install github:nix-community/namaka

# Run tests
namaka check tests/namaka

# Review/accept changed snapshots
namaka review tests/namaka
```

## Tests

- `build-default-package` - smoke test that the namaka wiring evaluates
- `has-default-package` - the flake exposes `packages.x86_64-linux.default`
