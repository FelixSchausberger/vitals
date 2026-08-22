{ flake }:
builtins.hasAttr "default" (flake.packages.x86_64-linux or { })
