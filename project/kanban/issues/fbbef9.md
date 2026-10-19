---
id: fbbef9
title: Tests that need memory64 hold to the backend's capability in WebKitGTK
type: bug
blocked_by: []
labels: [webgpu-spike, webkitgtk, memory64]
created: 2026-10-18T16:52:15Z
---

## What to build

The tests that need memory64 hold to what the browser's backend declares,
so that they pass in WebKitGTK, which ships memory64 turned off.

`tests webkit debug` (a23124) runs the web tests in WebKitGTK 2.54.1's
MiniBrowser. Its JavaScriptCore ships `useWasmMemory64=false`, so the web
backend does not declare `Capability::Memory64` there, and these tests fail
only in WebKitGTK (each also fails alone, on a page of its own):

- `wcmp` unit tests: `executor::thread_start_table::tests::` `it_reads_a_start_function_that_takes_an_i64`,
  `it_reads_a_start_function_that_takes_an_i32`,
  `it_fails_an_index_past_the_end_of_the_table`,
  `it_fails_an_empty_entry_with_wasmtimes_message`,
  `it_fails_a_start_function_of_another_type_with_wasmtimes_message` (their
  shared fixture compiles start functions with a 64-bit memory:
  `the backend does not support memory64`);
  `abi::transcode::tests::it_transcodes_with_64_bit_pointers_and_lengths`
  (`Unsupported(Memory64)`);
  `baseline::thread_builtins::it_starts_a_thread_whose_start_function_takes_an_i64_in_a_64_bit_memory`.
- `wcmp::baseline_composition`:
  `it_copies_strings_between_a_32_bit_and_a_64_bit_component`
  (`Unsupported { feature: "memory64" }`).
- `wcmp::conformance`: `it_passes_wasmtime_memory64`
  (`wasmtime/memory64.wast:24 component rejected: unsupported component
  feature: memory64`).
- `wcmp-wasm-core-web::web`:
  `it_refuses_a_bulk_copy_past_4_gib_where_multi_memory_is_off` (asserts
  `Capability::Memory64`) and `it_declares_what_chromium_implements`
  (`declares memory64`).

Safari 26 is in the same position (see `SuspendProviderKind`'s docs on older
browsers), so the fix serves Safari as well. A test that needs memory64
either skips where the backend lacks it, with the reason, or asserts the
refusal the polyfill gives there. The thread-start fixture should not need
memory64 for its 32-bit cases. The conformance corpus needs a way to name
the memory64 lines as unsupported where the capability is absent, rather
than a WebKit list.

## Acceptance criteria
- [x] Each test above passes in `tests webkit debug`, and still covers
      memory64 where the backend declares it (Chrome, native).
- [x] The tests that need memory64 say so where the backend lacks it,
      rather than passing silently.
- [x] `lint` passes, `tests all` is green, and `tests webkit debug` shows
      none of these tests failing.

## Review notes

- 2026-10-19 (implementer): the thread-start fixture keeps its values in
  a global rather than a 64-bit memory, so its five tests run without
  memory64. Each test that needs memory64 checks the refusal where the
  backend lacks it (`Unsupported(memory64)` at `Component::new`, or
  `Unsupported(Memory64)` from the runtime layer), through a new
  `declares` of the shared test backend. The conformance corpus gains
  `expected-failures.no-memory64.txt`, applied on any backend that does
  not declare memory64 (one line: `wasmtime/memory64.wast:24`).
  `it_declares_what_chromium_implements` became
  `it_declares_what_the_engine_of_the_page_implements`, which holds each
  engine to its own set and cites WebKitGTK 2.54.1's
  `WasmMemory64Enabled` default. A test helper (`tests/support/page_engine.rs`)
  tells the page's engine from its user agent. `tests webkit debug`: all
  eleven pass (1854 tests in 72 binaries; only 1429ec's two still failed
  in that run).

- 2026-10-19 (implementer): gates on the tip (kanban `de056af7`): `lint`
  passes, and `tests all -j 6` passes every lane, the WebKitGTK lanes
  among them (`webkit debug` 1854 tests in 103 s, `webkit no-provider`
  169 in 20 s, `fidelity webkit` 265 in 27 s).

- 2026-10-19 (peer review, round 1): ACCEPT, three non-blocking findings,
  answered: the memory64 delta refuses a line the web delta names too
  (test `it_rejects_a_memory64_line_the_web_delta_already_names`); the
  JavaScriptCore branch of the capability test says it pins WebKitGTK
  2.54.1's set, since Safari 26 has no JSPI; a long doc line rewrapped.

- 2026-10-19 (peer review, round 2): ACCEPT; the round-1 answers hold.
