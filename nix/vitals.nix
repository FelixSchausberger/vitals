# Vitals system health monitor - NixOS module
#
# Usage in NixOS:
#   imports = [ inputs.vitals.nixosModules.default ];
#   services.vitals.enable = true;
inputs:
{ config, lib, pkgs, ... }:
let
  cfg = config.services.vitals;
  inherit (pkgs.stdenv.hostPlatform) system;
in
{
  options.services.vitals = {
    enable = lib.mkEnableOption "vitals health monitoring daemon";

    package = lib.mkOption {
      type = lib.types.package;
      default = inputs.self.packages.${system}.daemon;
      defaultText = lib.literalExpression "inputs.vitals.packages.\${system}.daemon";
      description = "The vitals-daemon package to run.";
    };

    mode = lib.mkOption {
      type = lib.types.enum [
        "live"
        "mock"
      ];
      default = "live";
      description = ''
        Data source mode: `live` reads journald/systemd/procfs,
        `mock` serves deterministic test data.
      '';
    };
  };

  config = lib.mkIf cfg.enable {
    systemd.services.vitals-daemon = {
      description = "Vitals health monitoring daemon";
      after = [ "network.target" ];
      wantedBy = [ "multi-user.target" ];

      environment = {
        # Unix socket lives at $XDG_RUNTIME_DIR/vitals/daemon.sock
        XDG_RUNTIME_DIR = "/run/vitals";
        # Daemon address discovery file
        XDG_STATE_HOME = "/var/lib/vitals";
        # Score history persistence
        XDG_DATA_HOME = "/var/lib/vitals";
      };

      serviceConfig = {
        ExecStart = "${lib.getExe cfg.package} --mode ${cfg.mode}";
        Restart = "on-failure";
        RestartSec = 5;
        DynamicUser = true;
        RuntimeDirectory = "vitals";
        StateDirectory = "vitals";
        # ProtectSystem=strict mounts /run read-only; allow socket binding
        ReadWritePaths = [ "/run/vitals" ];
        NoNewPrivileges = true;
        ProtectSystem = "strict";
        ProtectHome = true;
        PrivateTmp = true;
      };
    };
  };
}
