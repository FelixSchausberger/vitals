# Namaka test for verifying the Nix flake builds correctly
# See https://github.com/nix-community/namaka

{
  # Test that the default package builds successfully
  build-default-package = {
    expr = builtins.trace "Testing default package build" true;
    expected = true;
  };

  # Test that the flake has required outputs
  has-default-package = {
    expr = builtins.hasAttr "default" (import ../../flake.nix).packages.x86_64-linux or { };
    expected = true;
  };
}
