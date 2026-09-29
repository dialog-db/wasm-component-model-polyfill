#!/usr/bin/env bash
# Regenerates every fixture under this directory from its sources. The
# `fixtures` menu command runs this script with the flake's `wasm-tools`,
# `wac`, and Rust toolchain on the PATH, so a rerun on the same system
# reproduces every output byte for byte. The fixtures README records
# what that claim covers.
#
# A fixture directory holds a WIT package and either one core module in
# WAT per world or one Rust crate per component. A WAT fixture becomes a
# component with `wasm-tools component embed` + `wasm-tools component
# new`; a Rust one is built by `cargo` and `wasm-tools component new`
# (see `rust_components`). Either kind composes with `wac plug` where it
# has a socket. The script writes `<fixture>.wast` next to the
# directory: the final component as a `(component binary ...)` directive
# followed by the hand-written `assertions.wast`. The conformance
# harness runs those `.wast` files like the vendored corpora.
set -euo pipefail
cd "$(dirname "$0")"

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
cargo_home=${CARGO_HOME:-$HOME/.cargo}
# The Rust toolchain ships the standard library's sources, and a panic
# in `alloc` or `core` names them by absolute path. On Nix that path
# carries a store hash that differs per platform and per nixpkgs
# revision, so it is remapped like the other two roots below.
sysroot=$(rustc --print sysroot)

# Build every component of a fixture that a language toolchain
# compiles rather than a hand-written WAT. Such a fixture holds its WIT
# under `wit/` and one directory per component, and its cargo metadata
# is spelled `cargo-workspace.toml`, `cargo-lock.toml`, and
# `<component>/cargo-manifest.toml` rather than `Cargo.toml` and
# `Cargo.lock`: the Nix source filter carries everything under `rust/`
# into the polyfill workspace's dependency bundle, and a manifest it
# finds there invalidates that bundle on every rebuild. This function
# assembles a cargo workspace under `$TMPDIR` from those files and
# builds it with the flake's Rust toolchain, leaving one component per
# member at `$work/<fixture>-<component>.wasm`.
#
# `RUSTFLAGS` remaps the build directory, the cargo registry, and the
# toolchain's sysroot out of the binary, so the output depends on
# neither where the build ran nor which machine ran it.
#
# The guests build for `wasm32-unknown-unknown` rather than for a
# `wasip2` or `wasip3` target. What these fixtures exercise is the
# canonical ABI, not a WASI host: nothing in them calls libc, and the
# binding layer is the same either way — `cabi_realloc` comes from
# `wit-bindgen-rt` over dlmalloc, and a `wasi:` world still reaches the
# component level through wit-bindgen's own bindings, so the
# `wasi-http` guest carries `wasi:http/types@0.3.0` as a component
# import from a `wasm32-unknown-unknown` build. A `wasip2` target would
# additionally link the standard library against `wasi:cli`,
# `wasi:filesystem`, `wasi:io`, and the rest, adding imports no
# assertion exercises and the polyfill's linker would have to be
# handed.
#
# `wit-bindgen`'s macro writes the component type into a custom
# section, so `wasm-tools component new` needs no `embed` step.
rust_components() { # fixture, component...
  local fixture=$1
  shift
  local dir="$work/$fixture" component
  mkdir -p "$dir"
  cp -R "$fixture/wit" "$dir/wit"
  cp "$fixture/cargo-workspace.toml" "$dir/Cargo.toml"
  cp "$fixture/cargo-lock.toml" "$dir/Cargo.lock"
  for component in "$@"; do
    mkdir -p "$dir/$component"
    cp "$fixture/$component/cargo-manifest.toml" "$dir/$component/Cargo.toml"
    cp -R "$fixture/$component/src" "$dir/$component/src"
  done
  (
    cd "$dir"
    CARGO_TARGET_DIR="$dir/target" \
      RUSTFLAGS="--remap-path-prefix=$work=/fixture --remap-path-prefix=$cargo_home=/cargo --remap-path-prefix=$sysroot=/rust" \
      cargo build --locked --release --workspace --target wasm32-unknown-unknown
  )
  for component in "$@"; do
    # `wasm-tools component new` validates its output against the
    # WebAssembly proposals at phase 4 and later. The asynchronous
    # component model is not one of them and the `wasi-http` fixture
    # is built on it, nor is the `error-context` type the
    # `error-reporter` fixture takes, so the encode step skips that
    # check and the component is validated separately with both turned
    # on.
    wasm-tools component new --skip-validation \
      "$dir/target/wasm32-unknown-unknown/release/$component.wasm" \
      -o "$work/$fixture-$component.wasm"
    wasm-tools validate -f cm-async,cm-error-context "$work/$fixture-$component.wasm"
  done
}

# Write a component binary as a `(component $name binary "...")`
# directive, 32 bytes per line so the file diffs line by line.
binary_directive() { # name, path
  echo "(component \$$1 binary"
  od -An -v -tx1 -w32 "$2" | sed -e 's/ /\\/g' -e 's/^/    "/' -e 's/$/"/'
  echo ")"
}

# Emit the `.wast` for one fixture: a header, the binary directive, and
# the hand-written assertions. A fixture whose component needs a gated
# feature keeps the `;;!` lines that turn it on in `features.wast`, and
# the header carries them, because the harness reads those lines only
# from the top of a file.
emit_wast() { # fixture, name, binary
  local fixture=$1
  {
    echo ";; Generated by \`fixtures\` from $fixture/. Do not edit by hand: the"
    echo ";; sources are the WIT, the guests, and assertions.wast in that directory."
    if [ -f "$fixture/features.wast" ]; then
      cat "$fixture/features.wast"
    fi
    echo
    binary_directive "$2" "$3"
    echo
    cat "$fixture/assertions.wast"
  } > "$fixture.wast"
}

# guest: one core module, one world, one component.
wasm-tools component embed --world guest guest/guest.wit guest/guest.wat -o guest/guest.core.wasm
wasm-tools component new guest/guest.core.wasm -o guest/guest.wasm
rm guest/guest.core.wasm
emit_wast guest guest guest/guest.wasm

# maps: one core module, one world, `map<string, u32>` in and out.
wasm-tools component embed --world maps maps/maps.wit maps/maps.wat -o maps/maps.core.wasm
wasm-tools component new maps/maps.core.wasm -o maps/maps.wasm
rm maps/maps.core.wasm
emit_wast maps maps maps/maps.wasm

# fixed-lists: one core module, one world, `list<T, N>` in and out.
wasm-tools component embed --world fixed-lists fixed-lists/fixed-lists.wit fixed-lists/fixed-lists.wat -o fixed-lists/fixed-lists.core.wasm
wasm-tools component new fixed-lists/fixed-lists.core.wasm -o fixed-lists/fixed-lists.wasm
rm fixed-lists/fixed-lists.core.wasm
emit_wast fixed-lists fixed-lists fixed-lists/fixed-lists.wasm

# composition: a plug that exports `math` and a socket that imports it,
# each a component of its own, then composed with `wac plug`.
wasm-tools component embed --world plug composition/math.wit composition/plug.wat -o composition/plug.core.wasm
wasm-tools component new composition/plug.core.wasm -o composition/plug.wasm
wasm-tools component embed --world socket composition/math.wit composition/socket.wat -o composition/socket.core.wasm
wasm-tools component new composition/socket.core.wasm -o composition/socket.wasm
rm composition/plug.core.wasm composition/socket.core.wasm
wac plug --plug composition/plug.wasm composition/socket.wasm -o composition/composed.wasm
emit_wast composition composed composition/composed.wasm

# rich: three components built by `cargo` and wit-bindgen against a
# world of records, variants, enums, flags, options, results, nested
# lists, strings, and two resources. `support` answers the shapes and
# owns the `tally` resource; `guest` imports it, exports its own
# `counter` resource, and exports the functions the assertions call;
# `driver` forwards each of those and drives `counter` from outside
# the component that defines it. Two `wac plug` steps join them, so
# every value crosses three component boundaries.
rust_components rich support guest driver
wac plug --plug "$work/rich-support.wasm" "$work/rich-guest.wasm" -o "$work/rich-linked.wasm"
wac plug --plug "$work/rich-linked.wasm" "$work/rich-driver.wasm" -o rich/rich.wasm
emit_wast rich rich rich/rich.wasm

# wasi-http: one component built by `cargo` and wit-bindgen against
# the WASI 0.3 packages under `wasi-http/wit/deps/`. It exports
# `wasi:http/handler@0.3.0` — an `async func` whose request and
# response carry a `stream<u8>` body and a `future` of trailers — and
# `drain`, the same machinery in a signature a directive can call.
rust_components wasi-http handler
cp "$work/wasi-http-handler.wasm" wasi-http/handler.wasm
emit_wast wasi-http handler wasi-http/handler.wasm

# wasi-http-same-instance: the `wasi-http` fixture as first written,
# kept as a tripwire on the spec. Its `drain` resolves a
# `future<result<_, error-code>>` whose two ends one instance holds,
# which the Component Model traps, as a temporary rule, for a payload
# that is not a number type.
rust_components wasi-http-same-instance handler
cp "$work/wasi-http-same-instance-handler.wasm" wasi-http-same-instance/handler.wasm
emit_wast wasi-http-same-instance handler wasi-http-same-instance/handler.wasm

# streams: one component built by `cargo` and wit-bindgen's async
# support. `words` answers with a `stream<string>` it writes a word at
# a time, and `checksum` reads a `stream<u32>` and resolves a
# `future<u64>` with its position-weighted checksum, both after the
# export has returned.
rust_components streams streams
cp "$work/streams-streams.wasm" streams/streams.wasm
emit_wast streams streams streams/streams.wasm

# stream-composition: two components built the same way. `counter`
# exports `count-up`, which streams the numbers from 1 to a count;
# `summer` imports it and exports `total`, which reads the stream to
# its end and returns the sum. `wac plug` joins them, so the stream
# crosses from one component's memory into the other's.
rust_components stream-composition counter summer
wac plug --plug "$work/stream-composition-counter.wasm" "$work/stream-composition-summer.wasm" -o stream-composition/composed.wasm
emit_wast stream-composition composed stream-composition/composed.wasm

# sync-wait: one component built by `cargo` and wit-bindgen, whose
# `async func` import and export are both bound synchronously. `total`
# calls the host's `host-echo-u32` once per key through a plain call
# that returns only once the host has answered, and sums the answers.
rust_components sync-wait waiter
cp "$work/sync-wait-waiter.wasm" sync-wait/sync-wait.wasm
emit_wast sync-wait waiter sync-wait/sync-wait.wasm

# stats: one component built by `cargo` and wit-bindgen, with a bug.
# `average` divides by the number of values without checking it, so an
# empty list panics and the guest traps, which poisons the store.
rust_components stats stats
cp "$work/stats-stats.wasm" stats/stats.wasm
emit_wast stats stats stats/stats.wasm

# error-reporter: one component built by `cargo` and wit-bindgen whose
# `describe` reads the debug message of an `error-context` it is
# handed. wit-bindgen's Rust generator lowers an `error-context` an
# export returns by borrowing its handle and drops the handle before
# the canonical ABI lifts it, so no Rust guest here returns one; the
# smoke test (`rust/wcmp-smoke`) writes the components that do by hand.
rust_components error-reporter reporter
cp "$work/error-reporter-reporter.wasm" error-reporter/error-reporter.wasm
emit_wast error-reporter reporter error-reporter/error-reporter.wasm

# deadline: one component built by `cargo` and wit-bindgen's async
# support, whose handler races the host's `fetch` against the host's
# `sleep` and cancels the call that lost. It imports an interface only
# a host supplies, so no `.wast` is written for it: the harness would
# stop at link. The smoke test (`rust/wcmp-smoke`) supplies that host.
rust_components deadline handler
cp "$work/deadline-handler.wasm" deadline/deadline.wasm

# The harness manifest: one `corpus_test!` per `.wast` under the corpus
# and the table the progress summary reads. A fixture's own directory
# holds only sources (its `assertions.wast` is not a runnable script), so
# those are skipped.
manifest=../../conformance/manifest.rs
files=$(cd .. && LC_ALL=C find . -name '*.wast' -not -path './fixtures/*/*' | sed 's|^\./||' | LC_ALL=C sort)
{
  echo "// Generated by \`fixtures\` from the files under \`tests/corpus/\`. Rerun"
  echo "// that command when a file is added or removed."
  echo
  for path in $files; do
    name=$(printf '%s' "${path%.wast}" | tr '[:upper:]' '[:lower:]' | sed -e 's/[^a-z0-9]\+/_/g' -e 's/^_//' -e 's/_$//')
    echo "corpus_test!(it_passes_$name, \"$path\");"
  done
  echo
  echo "/// Every corpus file with its text, for the progress summary."
  echo "const CORPUS_FILES: &[(&str, &str)] = &["
  for path in $files; do
    echo "    (\"$path\", include_str!(\"../corpus/$path\")),"
  done
  echo "];"
} > "$manifest"

echo "fixtures regenerated"
