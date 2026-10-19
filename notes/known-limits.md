# Known limits

The limits of each [backend](backends.md), grouped by cause.

## Engine defects

- V8 words two traps, an integer overflow and an invalid conversion to integer,
  with one message. The browser backend reads that trap as `TrapKind::Other`,
  never as a wrong kind. This is 75 of the browser's fidelity failures
  (`rust/wcmp-wasm-core-web/tests/fidelity/expected-failures.txt`, cited to V8's
  `message-template.h`).
- V8 checks the element segment before the array in `array.init_elem`, where the
  specification checks the array first. This is two fidelity failures (same
  file, cited to V8's `runtime-wasm.cc`).
- JavaScriptCore, the engine of Safari and WebKitGTK, words several pairs of
  traps with one message each:
  - an indirect call to a null entry and to a function of another signature
  - the two truncation traps
  - the bounds of a segment and of an array, in the array instructions that read
    a segment
  - in a module that waits, a refused atomic wait and an access out of the
    bounds of a memory

  The browser backend reads each as `TrapKind::Other` with the message, never as
  a wrong kind (`rust/wcmp-wasm-core-web/src/traps.rs`, cited to WebKit's
  source).

- A Wasmi built optimized with debug assertions overflows the native stack on a
  long run (wasmi-labs/wasmi#2039). The Wasmi crate docs give the profile
  setting that avoids it.
- Wasmtime's embedding API cannot carry a continuation reference
  (bytecodealliance/wasmtime#10248). The Wasmtime backend refuses such a
  crossing with `Error::Backend` (crate docs).

## Limits that the web embedding requires

- The JavaScript API carries no `v128`, no `exnref`, and no continuation
  reference, and it can change the bits of a NaN. The backend carries `v128`,
  `exnref`, and float values of an export through a generated carrier. Any other
  crossing of `v128` or `exnref`, such as the value of a global or a table
  element of that type, fails with `Error::TypeMismatch` (crate docs).
- The JavaScript API limits a table to 10,000,000 elements and a 64-bit memory
  to 2^37-1 pages. Every browser refuses a module above them. This is four
  fidelity failures, cited to the
  [JavaScript API](https://github.com/WebAssembly/spec/blob/608711107b7f1edb13efd57b7d79b49477462d36/document/js-api/index.bs#L2208-L2234).
- An atomic wait on the main thread of a page throws, as `Atomics.wait` does.
  This is two fidelity failures, cited to the threads proposal and ECMA-262.
- Chromium refuses a synchronous compile of a module above 8 MB on the main
  thread. `Component::new` compiles asynchronously, so this affects only the
  synchronous compile (crate docs).
- A shared memory needs a page with cross-origin isolation. The test lanes serve
  every page with the headers for it, so no lane shows a page without them
  (crate docs, card `f7eb9b`).

## Missing capabilities

- Wasmi lacks `gc`, `exceptions`, `function_references`, `threads`, and
  `stack_switching`. For lack of `gc`, 16 of 18 Zena scenarios stop at
  `Component::new` on Wasmi (`rust/wcmp/tests/zena/record.txt`). Together, the
  missing capabilities cost Wasmi 23 conformance directives (16 for
  `exceptions`, 5 for `gc`, and one each for `threads` and
  `function_references`), and 37 more that fail after them
  (`rust/wcmp/tests/corpus/expected-failures.wasmi.txt`).
- No browser ships `stack_switching`. WebKitGTK 2.54.0 ships memory64 off (card
  `629412`). In a browser without `gc`, `thread.new-indirect` fails with an
  internal error when it is called, not with `Unsupported(gc)` at
  `Component::new` (card `10acdb`, open).
- Wasmtime does not declare `host_suspension`, and declares `stack_switching`
  only where its compiler has it (crate docs).

## Backend limits

- The Wasmi backend allows at most 64 host calls inside one another. Wasmi
  itself does not bound a chain of host functions that call back into a guest,
  because each call runs Wasmi again on the native stack. The 65th call traps
  with `TrapKind::StackOverflow` before the host function runs (crate docs,
  `MAX_HOST_DEPTH` in `rust/wcmp-wasm-core-wasmi/src/store.rs`).
- Wasmi panics when it grows a 64-bit memory whose maximum is 2^48 pages
  (`wasmi_core` 2.0.0, `crates/core/src/memory/mod.rs:165`). The backend refuses
  to make such a memory, and refuses a host's growth of one. It cannot stop a
  guest's `memory.grow` on one, and on x86-64 that panic aborts the process. A
  host that runs untrusted memory64 modules on Wasmi checks their maxima itself
  (crate docs).
- Wasmi allocates and zero-fills every grown page (see [memory](memory.md)).
- The browser reaches only the first 4 GiB of a guest memory (see
  [memory](memory.md)).
- In the browser, a host function cannot resume a call, and a trap early in a
  thread started inside a host function loses its reason (see
  [suspend providers](suspend-providers.md#limits-of-host-suspension)).
- The Wasmtime backend does not build for the browser. On `wasm32` the crate is
  empty (crate docs).
