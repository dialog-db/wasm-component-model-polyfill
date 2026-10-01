---
id: 7cf3ff
title: Transcode strings without copying the whole source to the host
type: bug
blocked_by: [67f37c]
labels: [runtime-layer, PDD025]
created: 2026-09-30T20:41:24Z
---

## What to build

Found by the Wasmi lane work (67f37c) and confirmed by its independent review. `rust/wcmp/src/abi/transcode.rs` (`:44-46`, and `read()` at `:202`) copies the whole source string into a host `Vec` before transcoding. That is against PDD025's goals ("copies nothing natively where it reads"; "`Memory::copy` without a buffer on the host"). It is why `wasmtime/big-strings.wast` peaks at 4.1 GiB on Wasmtime and 9.2 GiB on Wasmi, so the Wasmi lane needs about 9 GiB for that one file, and a smaller host runs out of memory. In the browser, a multi-GiB string would also exhaust the polyfill's own wasm32 memory.

Validate through `with_bytes`, and copy with `Memory::copy` or in bounded chunks, so the host never holds the whole string.

## Acceptance criteria
- [ ] Transcoding holds at most a bounded chunk of the source on the host, on every backend.
- [ ] `wasmtime/big-strings.wast` peaks well below its current RSS on Wasmtime and Wasmi (record both numbers), and the `memory-hogs` group is narrowed if it can be.
- [ ] Transcoding results are unchanged: `tests all` passes with no record move.
- [ ] `lint` passes.

## Review notes


## Dispatch log

- 2026-09-30: pulled into the PDD025 thread at the owner's request (label added).
- 2026-09-30: run memory-heavy tests one at a time (the Wasmi lane's `memory-hogs` nextest group exists for this); this VM has 16 GiB. Card c91d3d renames the faithfulness suite to fidelity in parallel; expect a small merge in the flake and nextest config.
- 2026-09-30: implementor `card-7cf3ff-9af440f9` dispatched.
- 2026-10-01: implementor reported done at `e207b7647` (64 KiB chunks lent through `with_bytes`; copy-only ops validated chunk by chunk then moved with `Memory::copy`, no host buffer; 10 new unit tests). Peak RSS on `big-strings.wast`: Wasmtime 4125 → 34 MiB, Wasmi 9230 → 5134 MiB (the rest is Wasmi zero-fill); `memory-hogs` now applies only to the Wasmi lane. One record moved fail to pass: web `wasmtime/big-strings.wast:342,379` (the old cause was the polyfill exhausting its own wasm32 heap, not V8). `tests all` and `lint` green. Implementor paused. Reviewer `review-7cf3ff-7946adee` launched; branch delivered.
- 2026-10-01: reviewer `review-7cf3ff-7946adee` **accepted** at `e207b7647` (`tests all` 11 lanes and `lint` green; peak RSS measured itself: big-strings 29 MiB on Wasmtime, 5135 MiB on Wasmi; `memory-hogs` matches only under the Wasmi profile; every op checked against Wasmtime's libcalls at chunk splits, and no boundary changes a result; Latin-1 inflation from the end is safe; `Memory::copy` is memmove-safe on every backend; the web move of `big-strings.wast:342,379` to pass is consistent with the old cause being the polyfill's own wasm32 heap). Landed as `05bbca437` after the fidelity rename (`.config/nextest.toml` and `flake.nix` auto-merged). Follow-ups: e84e63. Revert artifact: `sandbox-guest/card-7cf3ff-9af440f9`.
