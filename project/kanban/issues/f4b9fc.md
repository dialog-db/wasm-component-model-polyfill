---
id: f4b9fc
title: A payload of another Rust type fails with a structured error, not an internal one
type: bug
blocked_by: []
labels: [typed-values]
created: 2026-10-16T18:37:54Z
---

## What to build

Follow-up from the second review of the PDD027 series. Two Rust types that stand for one WIT payload must not end in `Error::Internal`.

- **Scenario:** an export round-trips a host-created `StreamReader<String>`, and the host acquires it as returning `StreamReader<Box<str>>`. Both type-check against `stream<string>`.
- **Result:** piping the returned reader to a host consumer of `Box<str>` fails in `rust/wcmp/src/executor/host_pipe.rs` with "a host pipe's producer delivered items of another Rust type than its consumer", an internal error.
- **Rule:** CLAUDE.md asks for structured `Error` variants on runtime paths.

Refuse at the lift instead. When an end's record carries `EndPayload::Host` with another `TypeId` than the reader's `T`, fail the lift with `CopyCause::PayloadMismatch` (or the existing payload-mismatch cause), which `EndPayload::carries::<T>()` already allows.

## Acceptance criteria
- [ ] The scenario above fails at the lift with a structured payload-mismatch error, on both targets, and no internal error is raised.
- [ ] A reader whose Rust type is the producer's still crosses and pipes.
- [ ] `lint` passes, and `tests all` is green on both targets with the conformance summary unchanged.

