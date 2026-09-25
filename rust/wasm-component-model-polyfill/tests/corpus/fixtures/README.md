# Conformance fixtures

Every fixture here is a component the component toolchain built, not a
`.wast` written by hand. The `fixtures` menu command runs `build.sh`
with the flake's pinned tools and regenerates every output, including
`tests/conformance/manifest.rs`; a rerun on the same system writes the
same bytes, and "What the byte-stability claim covers" below says how
far that reaches. Each fixture directory holds only sources; the
`<fixture>.wast` beside it is generated, and the conformance harness
runs it like the vendored corpora.

A `.wast` here is the final component as a `(component $name binary
...)` directive followed by that fixture's `assertions.wast`.

## The fixtures

| Fixture                   | Sources                                                                 | Build                                                                          |
| ------------------------- | ----------------------------------------------------------------------- | ------------------------------------------------------------------------------ |
| `guest`                   | `guest/guest.wit` (world `guest`), `guest/guest.wat`                    | `wasm-tools component embed --world guest`, then `wasm-tools component new`    |
| `composition`             | `composition/math.wit` (worlds `plug` and `socket`), two `.wat` modules | each world as above, then `wac plug --plug plug.wasm socket.wasm`              |
| `maps`                    | `maps/maps.wit` (world `maps`), `maps/maps.wat`                         | as `guest`                                                                     |
| `fixed-lists`             | `fixed-lists/fixed-lists.wit`, `fixed-lists/fixed-lists.wat`            | as `guest`                                                                     |
| `rich`                    | `rich/wit/rich.wit`, three Rust crates                                  | `cargo build`, `wasm-tools component new`, then two `wac plug` steps           |
| `wasi-http`               | `wasi-http/wit/` (WASI 0.3 packages), one Rust crate                    | `cargo build`, then `wasm-tools component new`                                 |
| `wasi-http-same-instance` | as `wasi-http`, as first written                                        | as `wasi-http`                                                                 |

`build.sh` records the exact commands. The final `.wasm` of each
fixture is checked in next to its sources.

## The Rust fixtures

`rich`, `wasi-http`, and `wasi-http-same-instance` are built by a
language toolchain, so the binding layer is the one a real guest
carries: `cabi_realloc` from the allocator, wit-bindgen's lift and
lower code, and — for the two `wasi-http` fixtures — a `wasi:` world's
imports.

Their cargo metadata is spelled `cargo-workspace.toml`,
`cargo-lock.toml`, and `<component>/cargo-manifest.toml` rather than
`Cargo.toml` and `Cargo.lock`. The Nix source filter carries
everything under `rust/` into the polyfill workspace's dependency
bundle, and crane keeps every `Cargo.toml` it finds there in that
bundle's dummy source, so a manifest checked in under the corpus would
rebuild all four dependency bundles on every change. `build.sh`
assembles a cargo workspace under `$TMPDIR` from those files instead,
and the tree stays free of cargo metadata.

`wit-bindgen`'s macro writes the component type into a custom section,
so `wasm-tools component new` needs no `embed` step. It validates its
output against the WebAssembly proposals at phase 4 and later; the
asynchronous component model is not one of them, so `build.sh` skips
that check and validates each component separately with the feature
turned on.

The guests build for `wasm32-unknown-unknown` rather than for a
`wasip2` or `wasip3` target. What they exercise is the canonical ABI,
not a WASI host: nothing in them calls libc, and the binding layer is
the same either way — `cabi_realloc` comes from `wit-bindgen-rt` over
dlmalloc, and a `wasi:` world still reaches the component level
through wit-bindgen's own bindings, so the `wasi-http` guest carries
`wasi:http/types@0.3.0` as a component import from a
`wasm32-unknown-unknown` build. A `wasip2` target would additionally
link the standard library against `wasi:cli`, `wasi:filesystem`,
`wasi:io`, and the rest, adding imports no assertion exercises and the
polyfill's linker would have to be handed.

### `rich`

Three components against one world of records, variants, enums, flags,
options, results, nested lists, strings, and two resources:

- `support` exports `wcmp:rich/host-ops@0.1.0`: the far side of every
  shape, and the `tally` resource whose constructor, methods, and
  destructor run in the component that defines it.
- `guest` imports that interface, drives `tally` across the boundary,
  exports its own `counter` resource — a constructor, two methods, a
  static method over two borrows, and a destructor — and exports the
  functions the assertions call.
- `driver` forwards each of those and drives `counter` from outside
  the component that defines it, so the resource's exported entry
  points are reached through the canonical ABI rather than from
  inside the guest.

`wac plug` joins them in two steps, so every value the assertions
check has crossed three component boundaries in each direction.

The composed component keeps one import, the type-only
`wcmp:rich/shapes@0.1.0` instance that the worlds' `use` declarations
name. It asks the host for nothing, and the linker binds such an
import with no registration, as Wasmtime does.

The worlds and the assertions live in `rich/wit/rich.wit` and
`rich/assertions.wast`. The assertions call every export of the
composed component and check every value and both destructor counts.

### `wasi-http`

One component that implements `wasi:http/handler@0.3.0` as WASI 0.3
released it: the export is an `async func`, the request and response
carry their body as a `stream<u8>` and their trailers as a
`future<result<option<trailers>, error-code>>`. `wasi-http/wit/deps/`
holds the WASI 0.3 packages verbatim, copied from
`bytecodealliance/wasmtime`, `crates/wasi-http/src/p3/wit/deps/`, at
commit `358ee7665bff` — the revision this flake's `wasmtime-src` input
pins.

The component imports `wasi:http/types@0.3.0`, and the conformance
harness registers no host for it. The definition directive therefore
fails at link, a `deferred-feature` expectation in
`tests/corpus/expected-failures.txt`, and the assertions behind it are
`cascade` lines, which is how the corpus records a directive that
fails as bookkeeping after an earlier one. The repository test
`tests/baseline_wasi_http_handler.rs` supplies the interface through
the linker and calls both exports.

`wasi:http/handler@0.3.0` lives inside an exported instance, and a
`request` is a resource. The harness resolves an `invoke` against
root-level exports, as Wasmtime's wast runner does, and `wast` has no
syntax for a resource value, so no directive can hand the handler a
request. The world exports `drain` beside the handler for that reason:
the same body-and-trailers machinery in a signature a directive can
call, naming no `wasi:http` type. `drain` resolves a `future<u32>`
with the count of bytes it wrote, not the handler's
`future<result<_, error-code>>`. The Component Model traps, as a
temporary rule, a copy between two ends of a stream or future that one
instance holds when the payload is not a number type, and `drain`
holds both ends. A wit-bindgen guest can create only the future types
a function of its world names, so the world also exports `count`,
which takes a `future<u32>` and drops it. `drain` traps when the count
differs from the bytes it read back, and when the future is dropped
unwritten, so an empty input cannot pass on a missing count.

### `wasi-http-same-instance`

The `wasi-http` fixture as first written, kept as a tripwire on the
spec. Its `handler.wit`, `lib.rs`, cargo metadata, and `wit/deps/` are
the first version's, and its `drain` resolves a
`future<result<_, error-code>>` whose two ends the one instance holds.
The Component Model traps that copy because the payload is not a
number type. The spec marks that rule as temporary, so the fixture
records the day the rule is lifted: its `drain` then returns.

Its `handler.wasm` is byte for byte the first version of the
`wasi-http` fixture's. A panic location in the handler names its
source relative to the cargo workspace, as `handler/src/lib.rs`, so
the fixture's directory name does not reach the binary.

Like `wasi-http`, it imports `wasi:http/types@0.3.0`, so under the
harness its definition fails at link and its `drain` directives
cascade. `tests/corpus/expected-failures.txt` notes on those lines
that `drain` traps on the same-instance rule once a host supplies the
interface. The repository test `tests/baseline_wasi_http_handler.rs`
supplies it, calls `drain`, and asserts the trap, so that test fails
when the polyfill stops trapping the copy.

## What the byte-stability claim covers

Two runs of `fixtures` on the same machine produce identical bytes,
and the checked-in `.wasm` and `.wast` files are what a rerun writes,
so a stale fixture shows up as a dirty tree. That much has been
observed rather than assumed. It holds because the toolchain comes
from the flake, the dependency versions from each fixture's
`cargo-lock.toml`, and `RUSTFLAGS` remaps three absolute roots out of
the binary: the build directory under `$TMPDIR`, the cargo registry,
and the Rust toolchain's sysroot. The sysroot matters because the
toolchain ships the standard library's sources and a panic in `alloc`
or `core` names them by absolute path; on Nix that path carries a
store hash that differs per platform and per nixpkgs revision, so
without the remap the same sources would build to different bytes on
a different machine.

Four families of absolute path survive the remaps, and every Rust
fixture carries all four:

- `/rust/lib/rustlib/src/rust/library/...`, the standard library
  sources the toolchain ships, caught by the sysroot remap.
- `/cargo/registry/src/index.crates.io-<hash>/<crate>-<version>/...`,
  caught by the registry remap: `wit-bindgen-0.62.0` in every binary,
  and `futures-core-0.3.34` and `futures-util-0.3.34` in both
  `handler.wasm` files as well.
- `/rustc/ac68faa20c58cbccd01ee7208bf3b6e93a7d7f96/library/...`, which
  no remap here touches. The precompiled standard library the
  toolchain ships was built with that remap already applied upstream,
  keyed on the commit rustc was built from.
- `/rust/deps/dlmalloc-0.2.11/src/dlmalloc.rs`, likewise applied
  upstream: the Rust build remaps the dependencies it vendors to
  `/rust/deps`, which shares a prefix with the sysroot remap's `/rust`
  by coincidence rather than coming from it. `dlmalloc` is the
  allocator behind `cabi_realloc` on `wasm32-unknown-unknown`, so every
  Rust fixture reaches it.

The last two are machine-independent for the same reason as the first
two: they name a toolchain, not a filesystem. `rust-toolchain.toml`
pins the toolchain, and the commit hash and vendored-dependency paths
its build baked in read the same wherever that toolchain is installed.
The registry hash is the one cargo derives for the sparse index; a
build that fetched through the git protocol or from a vendored
registry would spell it differently and so produce different bytes.
Nothing else in the output is known to vary across platforms, but only
Linux on x86-64 has been run, so a first run elsewhere is a check to
make, not a guarantee already given.

`fixtures` is a devshell command rather than a Nix derivation, so it
reaches crates.io when it runs and needs the network. The flake pins
`wasm-tools`, `wac`, and the Rust toolchain; it does not pin
wit-bindgen. wit-bindgen 0.62.0 is content-pinned instead, by the
checksums in each fixture's `cargo-lock.toml`, which `build.sh`
enforces by passing `--locked`.

## The pinned tools

| Tool          | Version  | Where it is pinned                                          |
| ------------- | -------- | ----------------------------------------------------------- |
| `wasm-tools`  | 1.247.0  | `pkgs.wasm-tools` from the flake's nixpkgs                   |
| `wac`         | 0.11.0   | built from source in `flake.nix`                             |
| Rust          | stable   | `rust-toolchain.toml`, through the flake's `rustToolchain`   |
| `wit-bindgen` | 0.62.0   | each Rust fixture's `cargo-lock.toml`, by checksum           |

`wac` is built from its own source rather than taken from nixpkgs,
which carries 0.10.0. Two things are wrong with that release for this
use. It encodes a composition whose socket names a type from an
interface it imports — WIT's `use` at world level — as a root-level
type import with an `eq` bound, which the component model does not
allow when the type refers to another defined type, so the composed
`rich` component failed validation. And `wac plug` collected its plugs
into a `std` `HashMap` and emitted them in that map's iteration order,
which Rust randomises per run, so a multi-plug composition such as
`rich` was not byte-stable — upstream `1171a94`, "Make `wac plug`
output deterministic", fixed that after 0.10.1. 0.11.0 carries both
fixes. Drop the override in `flake.nix` when nixpkgs catches up.

## Spelling a value in an assertion

`wast` has no syntax for a `map` value or a fixed-length list value. A
directive spells a map as a list of two-element tuples, the map's
canonical-ABI layout, and a fixed-length list as a list; the harness
turns them into the polyfill's values wherever the invoked function
declares a `map` or a `list<T, N>`, for arguments and expected results
alike.

`wast` has no syntax for a resource handle at all, and the harness
invokes root-level exports only. A fixture that wants a resource
exercised has to drive it from inside a component, as `rich` does.
