{
  description = "System monitoring suite with daemon and TUI - Cargo workspace";

  nixConfig = {
    extra-substituters = [
      "https://cache.nixos.org"
      "https://nix-community.cachix.org"
      "https://cache.garnix.io"
    ];
    extra-trusted-public-keys = [
      "cache.nixos.org-1:6NCHdD59X431o0gWypbMrAURkbJ16ZPMQFGspcDShjY="
      "nix-community.cachix.org-1:mB9FSh9qf2dCimDSUo8Zy7bkq5CX+/rkCWyvRCYg3Fs="
      "cache.garnix.io:CTFPyKSLcx5RMJKfLo5EEPUObbA78b0YQ2DTCJXqr9g="
    ];
  };

  inputs = {
    nixpkgs.url = "github:nixos/nixpkgs/nixos-unstable";
    flake-parts.url = "github:hercules-ci/flake-parts";
    systems.url = "github:nix-systems/default";
    rust-overlay.url = "github:oxalica/rust-overlay";

    # Dev tools
    treefmt-nix.url = "github:numtide/treefmt-nix";
  };

  outputs =
    inputs:
    inputs.flake-parts.lib.mkFlake { inherit inputs; } {
      systems = import inputs.systems;
      imports = [
        inputs.treefmt-nix.flakeModule
      ];

      perSystem =
        { config
        , pkgs
        , system
        , ...
        }:
        let
          # Apply rust-overlay to pkgs
          pkgs = import inputs.nixpkgs {
            inherit system;
            overlays = [ inputs.rust-overlay.overlays.default ];
          };
          # Read workspace Cargo.toml
          workspaceToml = builtins.fromTOML (builtins.readFile ./Cargo.toml);

          # Dependencies for Rust system monitoring
          nonRustDeps = with pkgs; [
            # System libraries for systemd/journald integration
            pkg-config
            systemd
            # Additional dependencies for TUI
            fontconfig
            freetype
            # For HTTP/SSL support
            openssl
          ];

          # Runtime dependencies (available in PATH)
          runtimeDeps = with pkgs; [
            systemd # For systemctl, journalctl commands
            # Development and debugging tools
            cargo-nextest # Test runner
            cargo-watch # File watching
            prek # Git pre-commit hooks runner
            taplo # TOML formatter and linter
          ];

          # Common Rust package build configuration
          commonRustPackage = {
            src = ./.;
            cargoLock.lockFile = ./Cargo.lock;

            buildInputs = nonRustDeps;
            nativeBuildInputs = with pkgs; [
              pkg-config
              makeWrapper
            ];

            # For systemd integration
            PKG_CONFIG_PATH = "${pkgs.systemd.dev}/lib/pkgconfig:${pkgs.openssl.dev}/lib/pkgconfig";
            SYSTEMD_LIB_DIR = "${pkgs.systemd}/lib";
            SYSTEMD_LIBS = "systemd";
          };
        in
        {
          # Package: vitals-daemon
          packages.daemon = pkgs.rustPlatform.buildRustPackage (
            commonRustPackage
            // {
              pname = "vitals-daemon";
              version = workspaceToml.workspace.package.version;

              # Build only the daemon binary
              cargoBuildFlags = [
                "--bin"
                "vitals-daemon"
              ];
              cargoTestFlags = [
                "--bin"
                "vitals-daemon"
              ];

              # Wrap binary with runtime dependencies
              postInstall = ''
                wrapProgram $out/bin/vitals-daemon \
                  --prefix PATH : ${pkgs.lib.makeBinPath runtimeDeps}
              '';

              meta = with pkgs.lib; {
                description = "Vitals monitoring daemon - backend service";
                homepage = "https://github.com/schausberger/vitals";
                license = licenses.mit;
                mainProgram = "vitals-daemon";
              };
            }
          );

          # Package: vitals-tui
          packages.tui = pkgs.rustPlatform.buildRustPackage (
            commonRustPackage
            // {
              pname = "vitals-tui";
              version = workspaceToml.workspace.package.version;

              # Build only the TUI binary
              cargoBuildFlags = [
                "--bin"
                "vitals-tui"
              ];
              cargoTestFlags = [
                "--bin"
                "vitals-tui"
              ];

              # Wrap binary with runtime dependencies
              postInstall = ''
                wrapProgram $out/bin/vitals-tui \
                  --prefix PATH : ${pkgs.lib.makeBinPath runtimeDeps}
              '';

              meta = with pkgs.lib; {
                description = "Vitals monitoring TUI - terminal interface";
                homepage = "https://github.com/schausberger/vitals";
                license = licenses.mit;
                mainProgram = "vitals-tui";
              };
            }
          );

          # Package: vitals (CLI query tool)
          packages.cli = pkgs.rustPlatform.buildRustPackage (
            commonRustPackage
            // {
              pname = "vitals";
              version = workspaceToml.workspace.package.version;

              cargoBuildFlags = [
                "--bin"
                "vitals"
              ];
              cargoTestFlags = [
                "--bin"
                "vitals"
              ];

              meta = with pkgs.lib; {
                description = "Vitals CLI — query the health daemon";
                homepage = "https://github.com/schausberger/vitals";
                license = licenses.mit;
                mainProgram = "vitals";
              };
            }
          );

          # Default package: daemon, tui, and cli combined
          packages.default = pkgs.symlinkJoin {
            name = "vitals";
            paths = [
              config.packages.daemon
              config.packages.tui
              config.packages.cli
            ];
            meta = with pkgs.lib; {
              description = "Vitals system monitoring suite (daemon + TUI)";
              homepage = "https://github.com/schausberger/vitals";
              license = licenses.mit;
            };
          };

          # Systemd service package
          packages.systemd-service = pkgs.stdenv.mkDerivation {
            name = "vitals-systemd-service";
            version = workspaceToml.workspace.package.version;
            src = ./.;

            buildInputs = [ config.packages.daemon ];

            installPhase = ''
              mkdir -p $out/lib/systemd/system
              mkdir -p $out/lib/systemd/user
              mkdir -p $out/share/doc/vitals

              # System service (runs as root)
              substitute ${./daemon/systemd/vitals-daemon.service} $out/lib/systemd/system/vitals-daemon.service \
                --replace "/usr/local/bin/vitals-daemon" "${config.packages.daemon}/bin/vitals-daemon"

              # User service (runs as user)
              substitute ${./daemon/systemd/vitals-daemon-user.service} $out/lib/systemd/user/vitals-daemon.service \
                --replace "/usr/local/bin/vitals-daemon" "${config.packages.daemon}/bin/vitals-daemon"

              # Documentation
              cp ${./daemon/systemd/README.md} $out/share/doc/vitals/systemd-setup.md
            '';

            meta = with pkgs.lib; {
              description = "Systemd service files for Vitals daemon";
              homepage = "https://github.com/schausberger/vitals";
              license = licenses.mit;
            };
          };

          # Development shell with enhanced toolchain and aliases
          devShells.default = pkgs.mkShell {
            inputsFrom = [
              config.treefmt.build.devShell
            ];

            shellHook = ''
              # Setup development environment
              export RUST_SRC_PATH=${pkgs.rustPlatform.rustLibSrc}
              export PATH=${pkgs.lib.makeBinPath runtimeDeps}:$PATH

              # Development aliases for workspace commands
              alias dev='nix develop'
              alias run:daemon='cargo run --bin vitals-daemon'
              alias run:tui='cargo run --bin vitals-tui -- --daemon-url http://localhost:8080'
              alias test='cargo nextest run --workspace'
              alias test:daemon='cargo nextest run -p vitals-daemon'
              alias test:tui='cargo nextest run -p vitals-tui'
              alias test:core='cargo nextest run -p vitals-core'
              alias format='cargo fmt --all'
              alias lint='cargo clippy --workspace --all-targets -- -D warnings'
              alias build='nix build'
              alias build:daemon='nix build .#daemon'
              alias build:tui='nix build .#tui'
              alias watch:daemon='cargo watch -x "run --bin vitals-daemon"'
              alias watch:tui='cargo watch -x "run --bin vitals-tui"'

              echo "🩺 Vitals Cargo Workspace - Development Environment"
              echo ""
              echo "📦 Workspace Structure:"
              echo "  core/   - Shared data models and types"
              echo "  daemon/ - Backend daemon with HTTP API"
              echo "  tui/    - Terminal UI client"
              echo ""
              echo "🚀 Quick Start:"
              echo "  run:daemon  - Start daemon on :8080"
              echo "  run:tui     - Start TUI (connects to daemon)"
              echo ""
              echo "🧪 Testing:"
              echo "  test        - Run all workspace tests"
              echo "  test:daemon - Test daemon only"
              echo "  test:tui    - Test TUI only"
              echo "  test:core   - Test core only"
              echo ""
              echo "🛠️  Development:"
              echo "  format      - Format all code with rustfmt"
              echo "  lint        - Run clippy on workspace"
              echo "  watch:daemon - Watch and auto-restart daemon"
              echo "  watch:tui    - Watch and auto-restart TUI"
              echo ""
              echo "📦 Build with Nix:"
              echo "  build        - Build both daemon and TUI"
              echo "  build:daemon - Build daemon only"
              echo "  build:tui    - Build TUI only"
              echo "  treefmt      - Format code (Nix treefmt)"
              echo ""
              echo "📝 Systemd Installation:"
              echo "  sudo ./daemon/scripts/install-daemon.sh"
              echo "  ./daemon/scripts/diagnose-daemon.sh"
            '';

            buildInputs =
              nonRustDeps
              ++ runtimeDeps
              ++ [
                # Modern Rust debugger
                pkgs.bugstalker
              ];

            nativeBuildInputs =
              (with pkgs; [
                # Rust toolchain (stable with all components, except rustfmt)
                rustc
                cargo
                clippy
                rust-analyzer
                cargo-watch
                cargo-edit

                # Development and build tools
                pkg-config
                systemd.dev # For systemd/journal crate building
                dbus.dev # For D-Bus integration

                # Build system support
                cmake
                clang

                # Debugging tools
                gdb
                lldb
              ])
              ++ [
                # Nightly rustfmt for import ordering features
                (pkgs.rust-bin.selectLatestNightlyWith (toolchain: toolchain.rustfmt))
              ];

            # Environment variables for development
            PKG_CONFIG_PATH = "${pkgs.systemd.dev}/lib/pkgconfig:${pkgs.openssl.dev}/lib/pkgconfig:${pkgs.dbus.dev}/lib/pkgconfig";
            SYSTEMD_LIB_DIR = "${pkgs.systemd}/lib";
            SYSTEMD_LIBS = "systemd";
            RUST_BACKTRACE = "1";
            RUST_LOG = "debug";
          };

          # Code formatting
          treefmt.config = {
            projectRootFile = "flake.nix";
            programs = {
              nixpkgs-fmt.enable = true;
              rustfmt = {
                enable = true;
                package = pkgs.rustfmt;
              };
            };
          };
        };
    };
}
