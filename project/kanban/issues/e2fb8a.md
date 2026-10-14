---
id: e2fb8a
title: The downstream guard refuses every Cargo config override
type: bug
blocked_by: [994479]
labels: [runtime-layer, backlog-burndown-001-q4]
created: 2026-10-02T04:37:52Z
disposition: accepted
disposition_at: 2026-10-16T06:31:48Z
---

## What to build

From the independent review of 994479 (downstream checks), which bypassed the guard end to end in a throwaway clone:

- **Two config files.** The guard reads `rust/wcmp-downstream/.cargo/config.toml` and falls back to `.cargo/config` only when `config.toml` is absent; when both exist, Cargo 1.96 uses `.cargo/config`. A benign `config.toml` plus a `[source.crates-io]` replacement in `config` passed the guard, and the replacement took effect.
- **Config `include`.** Cargo 1.96 honours `include = [{ path = "x.toml" }]` on stable, and the guard never reads the included file.
- **Blind spot.** Neither the guard nor the lock check sees a `[source]` replacement or a `paths` override; a `paths` override builds a local fork while `Cargo.lock` keeps the crates.io source and checksum.
- **Path packages.** The lock check (`flake.nix:1060-1063`) allows a path package whose name matches a published crate at any path, for example a copy at `rust/wcmp-downstream/vendor/wcmp`.
- **Smaller.** Excluding `/tests/` from the `wcmp` package drops `tests/support/backend.rs`, which `src/runtime_layer.rs:46-48` includes under `cfg(test)` through `#[path]`, so the `.crate`'s own unit tests no longer compile. The `downstream lock --dry-run` restores the lock only on a normal exit. The downstream crate is not clippied on either target.

Simplest fix the reviewer suggests: refuse any `rust/wcmp-downstream/.cargo` directory outright, and allow path packages only at their known paths.

## Acceptance criteria
- [x] The guard refuses a `[source]` replacement, a `paths` override, a config `include` and the two-file case; `downstream-guard` holds each.
- [x] Path packages are allowed only at the consumer's and the published crates' own paths.
- [x] The packaged `wcmp` crate's unit tests compile, or the exclusion is narrowed.
- [x] The dry run restores the lock on any exit; the downstream crate is clippied.
- [x] `tests all` and `lint` pass.

## Review notes
- 2026-10-08 delivered (host session, backlog-burndown-001-q4), commit
  `fix(downstream): Refuse Cargo configuration and stray path packages`.
  - Guard: the consumer may keep no `.cargo` directory at all; the guard names
    its files. `downstream-guard` holds `config-patch`, `config-paths`,
    `config-source-replacement`, and the new `config-include` and
    `config-two-files` (benign `config.toml` plus a replacing `config`).
  - Path packages: `downstreamPathCheck` runs jq over `cargo metadata` in
    `downstream-build`. Every path package must sit at
    `rust/<consumer or published crate>/Cargo.toml`. `downstream-guard` runs it
    against a placed set (accepted) and a `vendor/wcmp` copy (refused). The
    name-only lock check stays.
  - Packaging: `exclude = ["/tests/*", "!/tests/support/"]`. The `package`
    output's `wcmp-0.1.0.crate` holds `tests/support/backend.rs` and no other
    file under `tests/`. Nothing else under `src/` reaches into `tests/` (no
    `include_bytes!`, no `wcmp_scenario`), so the exclusion is narrowed as the
    card allows; the packaged unit tests were not compiled.
  - Dry run: an `EXIT` trap puts the committed lock back on any exit, and
    `INT`, `TERM` and `HUP` exit 130 so the trap runs.
  - Clippy: `downstream-build` runs `cargo clippy --all-targets -D warnings`
    for the host and for `wasm32-unknown-unknown`; both pass.
- 2026-10-08 gate on the series tip (`fix(zena): Move the Zena input back
  from the fork to Zena`): `tests all` green on every lane (native debug and
  release 1730, web debug and release 1716, no-provider 169 on each target,
  Wasmi 1447, fidelity 264/267/264, Zena Browser 18/18, Native 18/18, Wasmi
  2/18, Wasmtime 18/18, demo 68/68), with no `unexpected:` or `stale
  expectation:` line, and the full `lint` green. Conformance: native
  2412/2443, browser 2407, Wasmi 2352, no provider 2356.
- 2026-10-08 context-free review, round 1: the dry run's restoring trap is
  now installed after the committed lock is copied, so a failed copy cannot
  overwrite `Cargo.lock`.
- 2026-10-08 context-free review, round 4: **accepted**, with no code finding.
