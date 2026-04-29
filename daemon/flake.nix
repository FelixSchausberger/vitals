{
  description = "Vitals daemon - Lightweight system health monitoring";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs = { self, nixpkgs, flake-utils, rust-overlay }:
    flake-utils.lib.eachDefaultSystem (system:
      let
        overlays = [ (import rust-overlay) ];
        pkgs = import nixpkgs {
          inherit system overlays;
        };

        # Use stable Rust
        rustToolchain = pkgs.rust-bin.stable.latest.default.override {
          extensions = [ "rust-src" "rust-analyzer" ];
        };

        # System libraries needed for compilation
        buildInputs = with pkgs; [
          systemd
          systemd.dev
        ];

        nativeBuildInputs = with pkgs; [
          rustToolchain
          pkg-config
        ];

        # Wrap prek with SSL certificate environment variables for WSL
        wrappedPrek = pkgs.symlinkJoin {
          name = "prek-wrapped";
          paths = [ pkgs.prek ];
          buildInputs = [ pkgs.makeWrapper ];
          postBuild = ''
            wrapProgram $out/bin/prek \
              --set SSL_CERT_FILE "/etc/ssl/certs/ca-bundle-enhanced.crt" \
              --set CURL_CA_BUNDLE "/etc/ssl/certs/ca-bundle-enhanced.crt" \
              --set NIX_SSL_CERT_FILE "/etc/ssl/certs/ca-bundle-enhanced.crt" \
              --set REQUESTS_CA_BUNDLE "/etc/ssl/certs/ca-bundle-enhanced.crt"
          '';
        };

      in
      {
        packages = {
          default = pkgs.rustPlatform.buildRustPackage {
            pname = "vitals-daemon";
            version = "0.1.0";

            src = ./.;

            cargoLock = {
              lockFile = ./Cargo.lock;
            };

            nativeBuildInputs = nativeBuildInputs;
            buildInputs = buildInputs;

            # Set environment variables for systemd
            PKG_CONFIG_PATH = "${pkgs.systemd.dev}/lib/pkgconfig";

            meta = with pkgs.lib; {
              description = "Lightweight system health monitoring daemon";
              homepage = "https://github.com/schausberger/vitals-daemon";
              license = licenses.mit;
              maintainers = [ ];
            };
          };
        };

        devShells.default = pkgs.mkShell {
          buildInputs = buildInputs ++ [
            rustToolchain
            pkgs.cargo-nextest
            pkgs.cargo-watch
            pkgs.pkg-config
            pkgs.uv
            wrappedPrek
          ];

          shellHook = ''
            export RUST_SRC_PATH="${rustToolchain}/lib/rustlib/src/rust/library"
            export PKG_CONFIG_PATH="${pkgs.systemd.dev}/lib/pkgconfig"
            export SYSTEMD_LIB_DIR="${pkgs.systemd}/lib"
            export RUST_BACKTRACE=1
            export RUST_LOG=debug

            # SSL certificate configuration for WSL (for prek/Python tools)
            export SSL_CERT_FILE="/etc/ssl/certs/ca-bundle-enhanced.crt"
            export CURL_CA_BUNDLE="/etc/ssl/certs/ca-bundle-enhanced.crt"
            export NIX_SSL_CERT_FILE="/etc/ssl/certs/ca-bundle-enhanced.crt"
            export REQUESTS_CA_BUNDLE="/etc/ssl/certs/ca-bundle-enhanced.crt"

            echo "🩺 Vitals Daemon Development Environment"
            echo ""
            echo "Available commands:"
            echo "  cargo build           - Build the daemon"
            echo "  cargo run             - Run daemon mode"
            echo "  cargo run -- --once   - Run one-shot health check"
            echo "  cargo run -- --once --explain - Show detailed breakdown"
            echo "  cargo nextest run     - Run tests with nextest"
            echo "  cargo watch -x run    - Watch and auto-rebuild"
            echo "  prek run --all-files  - Run pre-commit hooks"
            echo ""
          '';
        };

        # Apps for easy running
        apps = {
          default = {
            type = "app";
            program = "${self.packages.${system}.default}/bin/vitals-daemon";
          };
        };
      }
    );
}
