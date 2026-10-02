# Copyright 2026 The Dialog DB Project
#
# This Source Code Form is subject to the terms of the Mozilla Public
# License, v. 2.0. If a copy of the MPL was not distributed with this
# file, You can obtain one at https://mozilla.org/MPL/2.0/.

# The systems this flake builds for, read by flake-utils'
# `eachDefaultSystem` here and in katsuobushi. It is nix-systems' default
# list without x86_64-darwin, which nixpkgs dropped in 26.11: importing
# nixpkgs for it throws, and `nix flake check` evaluates every system.
[
  "aarch64-darwin"
  "aarch64-linux"
  "x86_64-linux"
]
