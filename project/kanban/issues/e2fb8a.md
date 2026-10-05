---
id: e2fb8a
title: The downstream guard refuses every Cargo config override
type: bug
blocked_by: [994479]
labels: [runtime-layer, backlog-burndown-001-q4]
created: 2026-10-02T04:37:52Z
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
- [ ] The guard refuses a `[source]` replacement, a `paths` override, a config `include` and the two-file case; `downstream-guard` holds each.
- [ ] Path packages are allowed only at the consumer's and the published crates' own paths.
- [ ] The packaged `wcmp` crate's unit tests compile, or the exclusion is narrowed.
- [ ] The dry run restores the lock on any exit; the downstream crate is clippied.
- [ ] `tests all` and `lint` pass.

## Review notes

