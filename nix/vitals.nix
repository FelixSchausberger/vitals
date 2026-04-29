# Vitals system health monitor - NixOS module
#
# Usage in NixOS (headless server):
#   imports = [ inputs.vitals.nixosModules.default ];
#   services.vitals.enable = true;
#   services.vitals.headless = true;
#
# For home-manager integration, add to your HM config:
#   imports = [ inputs.vitals.homeManagerModules.default ];

{ inputs
, pkgs
, config
, lib
, ...
}:
let
  inherit (pkgs.stdenv.hostPlatform) system;
  daemonPkg = inputs.vitals.packages.${system}.daemon;
in
{
  options.services.vitals = {
    enable = lib.mkOption {
      type = lib.types.bool;
      default = false;
      description = "Enable vitals health monitoring daemon";
    };

    headless = lib.mkOption {
      type = lib.types.bool;
      default = false;
      description = "Run on a headless server (use default.target instead of graphical-session.target)";
    };
  };

  config = lib.mkIf config.services.vitals.enable {
    systemd.user.services.vitals-daemon = {
      Unit = {
        Description = "Vitals health monitoring daemon";
        After = [ (if config.services.vitals.headless then "default.target" else "graphical-session.target") ];
      };
      Service = {
        Type = "simple";
        ExecStart = "${daemonPkg}/bin/vitals-daemon";
        Restart = "on-failure";
        RestartSec = 5;
      };
      Install.WantedBy = [ (if config.services.vitals.headless then "default.target" else "graphical-session.target") ];
    };
  };
}
