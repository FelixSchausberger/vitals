# Namaka Tests

This directory contains Nix flake tests using [namaka](https://github.com/nix-community/namaka).

## Running Tests

```bash
# Install namaka
nix profile install github:nix-community/namaka

# Run tests
namaka check tests/namaka
```

## Test Files

- `build-test.nix` - Tests that the flake builds correctly
