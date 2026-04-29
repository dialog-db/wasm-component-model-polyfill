# Test Macros

The polyfill's test regime, as established by [PDD001], must run the same test
source on two targets — a native host and `wasm32-unknown-unknown` inside a
real browser — and must do so without ceremony at the call site. It must also
let contributors describe Wasm and Component-format inputs *next to* the tests
that exercise them, so that a test reads like an executable specification
rather than a fixture-management exercise.

This document describes a small family of macros that satisfy both
requirements. Together they form the in-tree authoring surface that every
other piece of the polyfill's test work — the wasip2 seed corpus, the
wasip3 implementation backlog, the conformance suite — will be written
against.

## Goals

- A single attribute lets a contributor mark an `async` test as cross-target,
  with no `cfg` boilerplate at the call site, so that the same source
  participates in `test:native:*` and `test:web:*` without duplication.
- Wasm Core modules and Component-format binaries can be expressed inline at
  the test site in [WebAssembly Text Format][WAT], assembled at compile time,
  and bound to ordinary Rust constants that flow into the polyfill's APIs.
- Mistakes — a syntax error in the WAT, a `(module …)` passed to a macro
  expecting a component, a sync function annotated with the async attribute —
  surface as compile errors at the call site, not at runtime.
- The macros are reusable across every crate in the workspace, so that future
  test crates inherit the same vocabulary without re-inventing it.

## Non-goals

- This document does not establish a sync cross-target test attribute. Sync
  tests use the language's built-in `#[test]`. The cross-target story exists
  to handle the async runtime split between `tokio` and
  `wasm_bindgen_test`; sync code does not need it.
- This document does not establish a WIT-driven binding generator. WIT-aware
  code generation is the subject of a separate workstream
  ([PDD003]'s "host-binding code generation" item) and is out of scope here.
- This document does not establish fixture-loading helpers — `include_bytes!`
  and the inline-assembly macros below are sufficient for the testing the
  polyfill needs at this stage. A larger fixture story may be revisited if
  pre-built component binaries enter the corpus.
- This document does not commit the inline-assembly macros to a public,
  consumer-facing API. They are a workspace-internal authoring tool;
  downstream exposure is out of scope here.
- This document does not address how host-side Component Model behaviour is
  asserted on (the polyfill's runtime APIs themselves are described in
  [PDD002] and [PDD003]); it only addresses the macros that surround such
  assertions.

## The Cross-Target Test Attribute

A single attribute marks a test as cross-target. The attribute takes no
arguments. The annotated function must be `async`; the macro rejects sync
functions with a clear compile error pointing the contributor at the
language's built-in `#[test]`. On `cfg(not(target_arch = "wasm32"))` the
expansion delegates to `tokio`'s async test attribute; on
`cfg(target_arch = "wasm32")` it delegates to `wasm_bindgen_test`. The
contributor never writes `cfg` themselves.

This attribute is intentionally narrower than upstream cross-target test
helpers in the Rust Wasm ecosystem — it does not stand up servers, it does
not allocate per-test resources, and it does not introduce custom timeouts.
Those capabilities, when needed, are layered on top of the test body, not
baked into the attribute.

## Inline Wasm and Component Assembly

The Component Model has two binary formats — Wasm Core and the Component
format — that share an `\0asm` preamble and differ in the four-byte version
word that follows it. The polyfill's tests need to author both. Two
function-like macros cover the surface:

- A *core-module* macro accepts a string literal containing WAT for a
  `(module …)` and returns a `&'static [u8]` of the assembled core-module
  binary.
- A *component* macro accepts a string literal containing WAT for a
  `(component …)` and returns a `&'static [u8]` of the assembled
  component binary.

Both macros are backed by the upstream [`wat`] assembler, which already
understands both grammars. Splitting the surface in two — rather than
exposing a single overloaded macro — is a deliberate readability choice:
the macro name documents the *intent* of the binary at the call site, and a
contributor who accidentally pastes a `(module …)` into the component macro
(or vice versa) gets a compile error rather than a binary that will fail
mysteriously in the runtime.

To make that guarantee concrete, the macros validate the assembled output
against the binary header before emitting code: if the version word does
not match the macro's declared kind, the compile error names both the
expected kind and the actual kind, and points the contributor at the other
macro. WAT parse errors are likewise surfaced as compile errors anchored at
the macro call site, so that line/column information from the assembler is
preserved in the contributor's editor.

The macros run entirely at compile time. The assembled bytes are emitted
into the binary as ordinary byte-array literals, so the produced constants
can be used in `const` context and incur no runtime cost beyond a slice
reference.

## Errors at the Source

A design through-line of all three macros is that misuse surfaces at compile
time, with the diagnostic anchored at the macro invocation. This applies to:

- A syntax error in the WAT (assembler error preserved verbatim).
- A binary-kind mismatch between the macro and the assembled output.
- A sync function annotated with the cross-target async attribute.
- Any unexpected argument passed to the cross-target attribute, which takes
  none.

This is what makes the macros suitable as the polyfill's authoring surface:
contributors get the same feedback loop they expect from the rest of the
Rust toolchain, and the test suite never grows fixtures whose existence
depends on conventions a code review must enforce.

## User Stories

**As a contributor writing a new polyfill test**, I want to mark the test
once and have it execute on both native and browser targets, so that I do
not maintain two copies of the same test or remember which target a given
test runs against.

> The contributor writes `#[wcmp_macros::test] async fn …`. They run
> `test:native:debug` and the test executes under `tokio`; they run
> `test:web:debug` and the same source is compiled to
> `wasm32-unknown-unknown`, bundled by `wasm-bindgen-test`, and executed
> in a headless browser.

**As a contributor specifying a small, hand-written component for a
focused test**, I want the component's source to live next to the
assertions, so that a future reader can see the input and the expected
behaviour without leaving the file.

> The contributor writes the component's WAT inside a `component!(...)`
> invocation bound to a `const`, then passes that constant into the
> polyfill's `Component::new` (or equivalent) in the test body. No
> `tests/fixtures/` lookup, no build-script step, no `.wasm` blob in the
> repository — the byte string of the component is part of the test
> source.

**As a contributor catching themselves in a mistake**, I want to learn
about the mistake from the compiler, not from a confusing runtime
failure inside the polyfill.

> The contributor pastes a `(module …)` into `component!(...)`. The
> compiler reports that the WAT assembled to a core module rather than a
> component and points them at `wasm!`. They fix the call site without
> ever building the test binary.

**As a maintainer reviewing a test added by someone else**, I want to
read the test as a single self-contained artifact, so that I can judge
its scope and correctness in one pass.

> The maintainer opens a test file. The cross-target attribute, the
> inline component definition, and the assertions are all visible
> together. They do not have to cross-reference fixture directories,
> build scripts, or `cfg`-guarded helpers to understand what the test
> exercises.

## References

- [PDD001] — the development environment that establishes cross-target
  testing on native and `wasm32-unknown-unknown` and references the
  upstream cross-target `#[test]` macro that inspired this document.
- [PDD002] — the polyfill's relationship to [`wasm_runtime_layer`] and
  [`wasm_component_layer`], whose APIs these macros exist to exercise in
  tests.
- [PDD003] — the compatibility outlook whose implementation checklist
  these macros' tests will progressively turn green.
- [WAT] — the WebAssembly Text Format the inline-assembly macros consume.
- [`wat`] — the upstream assembler the inline-assembly macros invoke at
  compile time.
- [wasm-bindgen-test] — the browser-targeting test harness the
  cross-target attribute delegates to on `wasm32-unknown-unknown`.
- [tokio] — the async runtime the cross-target attribute delegates to on
  native targets.
- [Dialog DB `#[test]` macro] — the upstream cross-target test attribute
  whose pattern this document's attribute is inspired by.

[PDD001]: ./PDD001%20Development%20Environment.md
[PDD002]: ./PDD002%20Ecosystem%20Foundation.md
[PDD003]: ./PDD003%20Compatibility%20Outlook.md
[WAT]: https://webassembly.github.io/spec/core/text/index.html
[`wat`]: https://docs.rs/wat
[`wasm_runtime_layer`]: https://github.com/DouglasDwyer/wasm_runtime_layer
[`wasm_component_layer`]: https://github.com/DouglasDwyer/wasm_component_layer
[wasm-bindgen-test]: https://rustwasm.github.io/docs/wasm-bindgen/wasm-bindgen-test/index.html
[tokio]: https://tokio.rs
[Dialog DB `#[test]` macro]: <https://github.com/dialog-db/dialog-db/blob/00c7bc5fa8ea187da7abda27c2a0a8edbd8c05ed/rust/dialog-common/src/lib.rs#L132>
