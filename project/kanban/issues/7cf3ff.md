---
id: 7cf3ff
title: Transcode strings without copying the whole source to the host
type: bug
blocked_by: [67f37c]
labels: [runtime-layer]
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

