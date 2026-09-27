---
id: fed20f
title: The smoke test shows cancellation, a poisoned store, and error contexts
type: chore
blocked_by: [c3c149, 4e6441, 45ccc2, f3a070]
labels: [PDD023, smoke, concurrency]
created: 2026-09-26T23:12:28Z
---

## What to build
The end-to-end smoke test (`rust/wcmp-smoke`, run by `tests smoke native` and `tests smoke check`) has no story about cancellation, error contexts, or a poisoned store. Add stories that show what the PDD023 work enables. Write each one as the existing stories are written: a `Story` with a chapter, a title, and a goal that a reader understands without the design, and a step that returns what it proved. Put them in a new chapter (for example "Failure and cancellation"), or in the existing chapters where they fit better.

Cover at least these shapes. Each follows one of PDD023's user stories:

- **A guest cancels a slow host call.** A guest's handler awaits a host `async` function whose future waits on a timer the store knows nothing about. When the guest's own deadline fires first, its runtime calls `subtask.cancel`. The host future is dropped in the next turn, the guest reads `CANCELLED_BEFORE_RETURNED`, a borrow it lent to the call comes back, and the handler answers with a timeout. The story checks that the host future was dropped.
- **A trap loses the store.** One export traps, for example on a divide by zero. `Func::call` returns the trap. A call of another export of the same store fails with "cannot enter component instance". The story builds a new store and calls the export there successfully.
- **An error passes between components.** A storage component fails a write and returns an `error-context` in a `result`. The caller component reads the debug message with `error-context.debug-message`, and then returns the same error context to the host. The host gets `Val::ErrorContext` and passes it to a third component, which reads the same message. The story turns on `wasm_component_model_error_context`, and its goal says the gate is off by default.
- **A guest thread stops when its caller cancels.** A worker thread waits with the cancellable form of `thread.yield`. The caller cancels, the yield returns 1, the worker calls `task.cancel`, and the caller reads `CANCELLED_BEFORE_RETURNED`. The PDD's user story is a C library that uses pthreads and wit-bindgen.

Build the fixtures from real toolchains where the toolchain supports the shape, as the "Real toolchains, real types" chapter does. Use the menu's `fixtures` command, and put the sources under `rust/wasm-component-model-polyfill/tests/corpus/fixtures/`. wit-bindgen's Rust runtime calls `subtask.cancel` when a guest drops a pending import future and has an `ErrorContext` type, so the first and third shapes should build from Rust. The flake supplies no C toolchain today. A shape that no toolchain in the flake can build may be written by hand in WAT inside the smoke crate, as `MODULE_CONSUMER` and `MEMORY64_COMPOSITION` are. Name each such shape in the report and say why. If a fixture needs a C toolchain added to the flake, report its cost instead of adding it.

Every story must pass natively on the x86_64 Linux host and in the flake's Chromium. A story whose shape needs a stack switch must, in a browser without JSPI, state the documented outcome in its goal. It must not fail silently.

## Acceptance criteria
- [ ] Smoke stories show a guest cancelling a slow host call, a trap losing the store, an error context passing between components and through the host, and a guest thread stopping on a cancellable yield, each with a goal a reader understands without the design.
- [ ] Any shape written in WAT instead of built by a toolchain is named and explained in the report.
- [ ] `tests smoke native` and `tests smoke check` pass with the new stories, and the story count in any smoke summary or documentation is updated.
- [ ] `lint` passes and `tests all` is green in all four states.


## Dispatch log
- 2026-09-27T13:39:22Z dispatched implementor `card-fed20f-5d361827` from code tip `610f4ba99` (all ten implementation cards landed; composed-tip gate green; implement session, PDD023 thread, budget 3, full gates).
