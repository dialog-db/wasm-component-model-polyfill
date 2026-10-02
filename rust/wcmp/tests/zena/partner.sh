#!/usr/bin/env bash
# Copyright 2026 The Dialog DB Project
#
# This Source Code Form is subject to the terms of the Mozilla Public
# License, v. 2.0. If a copy of the MPL was not distributed with this
# file, You can obtain one at https://mozilla.org/MPL/2.0/.

# Builds one Rust partner of a Zena scenario into a component. The
# flake runs this script inside the build sandbox, once per partner,
# with this workspace's Rust toolchain and `wasm-tools` on the PATH and
# the partner's crates vendored, so the partner is built at test time
# and never committed. The build takes nothing from the Zena toolchain,
# so a move of the Zena pin leaves the partner's bytes as they were.
#
# Arguments: the scenario's source directory, the partner's name, the
# cargo configuration that points the crates.io source at the vendored
# crates, and the output directory.
#
# A partner is a directory `<partner>/` in the scenario that holds a
# crate: `cargo-manifest.toml`, `cargo-lock.toml`, and `src/`. The crate
# is a `cdylib` that binds the scenario's WIT world with `wit-bindgen`
# (`path: "../wit"`), so the partner and the Zena programs compile
# against one contract. The cargo metadata is not named `Cargo.toml`
# and `Cargo.lock` in the tree, for the reason the conformance fixtures
# give: the Nix source filter carries everything under `rust/` into the
# polyfill workspace's dependency bundle, and a manifest there would
# rebuild that bundle on every change. The script assembles the crate
# under the build directory with its usual names instead.
#
# The partner builds for `wasm32-unknown-unknown`, as the Rust fixtures
# of the conformance suite do: a scenario exercises the Canonical ABI
# across a link, not a WASI host. `wit-bindgen` writes the component
# type into a custom section, so `wasm-tools component new` needs no
# `embed` step. It validates only the proposals at phase 4 and later,
# which leaves out the asynchronous Component Model, so the script
# skips that check and validates with `cm-async` turned on.
#
# The output holds the partner the way the Zena build holds a program,
# so the steps after this one read both alike:
#
# - `<partner>.status`: `0`. A partner that does not build fails the
#   build instead: it is this repository's code, and its failure is not
#   an outcome of Zena.
# - `<partner>.log`: empty.
# - `<partner>.wasm`: the component.
set -euo pipefail
shopt -s nullglob

scenario=$1
partner=$2
vendor_config=$3
out=$4

source_dir="$scenario/$partner"
for file in cargo-manifest.toml cargo-lock.toml src; do
  if [ ! -e "$source_dir/$file" ]; then
    echo "partner: $source_dir holds no $file" >&2
    exit 1
  fi
done

work=$(mktemp -d)
cp -R "$scenario/wit" "$work/wit"
mkdir -p "$work/$partner"
cp "$source_dir/cargo-manifest.toml" "$work/$partner/Cargo.toml"
cp "$source_dir/cargo-lock.toml" "$work/$partner/Cargo.lock"
cp -R "$source_dir/src" "$work/$partner/src"
chmod -R u+w "$work"

(
  cd "$work/$partner"
  CARGO_HOME="$work/cargo-home" \
    CARGO_TARGET_DIR="$work/target" \
    RUSTFLAGS="--remap-path-prefix=$work=/partner" \
    cargo build --config "$vendor_config" --offline --locked --release \
    --target wasm32-unknown-unknown
)

modules=("$work"/target/wasm32-unknown-unknown/release/*.wasm)
if [ ${#modules[@]} -ne 1 ]; then
  echo "partner: $partner built ${#modules[@]} core modules, not one" >&2
  exit 1
fi

mkdir -p "$out"
wasm-tools component new --skip-validation "${modules[0]}" -o "$out/$partner.wasm"
wasm-tools validate -f cm-async "$out/$partner.wasm"
echo 0 >"$out/$partner.status"
: >"$out/$partner.log"
echo "partner: $partner: built"
