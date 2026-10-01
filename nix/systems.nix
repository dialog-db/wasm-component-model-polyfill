# The systems this flake builds for, read by flake-utils'
# `eachDefaultSystem` here and in katsuobushi. It is nix-systems' default
# list without x86_64-darwin, which nixpkgs dropped in 26.11: importing
# nixpkgs for it throws, and `nix flake check` evaluates every system.
[
  "aarch64-darwin"
  "aarch64-linux"
  "x86_64-linux"
]
