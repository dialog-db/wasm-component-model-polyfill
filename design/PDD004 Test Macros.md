# Test Macros

The test regime of [PDD001] runs the same test source on two targets: a
native host and `wasm32-unknown-unknown` inside a real browser. It must do so
without ceremony at the call site. It must also let a contributor write Wasm
and Component inputs next to the tests that use them, so that a test reads
like an executable specification.

This document describes a small family of macros that meet both needs. They
form the authoring surface that every test in the polyfill is written
against.

## Goals

- One attribute marks an `async` test as cross-target. The same source runs
  under `test:native:*` and `test:web:*` with no `cfg` code at the call site.
- A contributor can write a Wasm Core module or a Component binary inline in
  [WebAssembly Text Format][WAT]. The macro assembles it at compile time and
  binds it to an ordinary Rust constant.
- Mistakes surface as compile errors at the call site. Examples are a syntax
  error in the WAT, a `(module …)` passed to the component macro, and a
  synchronous function marked with the asynchronous attribute.
- Every crate in the workspace can use the macros.

## Non-goals

- A synchronous cross-target test attribute. Synchronous tests use the
  built-in `#[test]`. The cross-target attribute exists to bridge the
  asynchronous runtime split between `tokio` and `wasm_bindgen_test`.
- A WIT-driven binding generator.
- Fixture-loading helpers. `include_bytes!` and the inline macros are enough.
- A public, consumer-facing API for the inline macros. They are a workspace
  authoring tool.
- The polyfill's runtime APIs. The macros only surround the assertions.

## The Cross-Target Test Attribute

One attribute marks a test as cross-target. The attribute takes no
arguments. The marked function must be `async`. If the function is
synchronous, the macro reports a compile error that points at the built-in
`#[test]`. On `cfg(not(target_arch = "wasm32"))` the attribute expands to
the `tokio` asynchronous test attribute. On `cfg(target_arch = "wasm32")` it
expands to `wasm_bindgen_test`. The contributor never writes `cfg`.

The attribute is narrower than similar helpers in the ecosystem. It does not
start servers, allocate per-test resources, or set timeouts. A test body adds
those when it needs them.

## Inline Wasm and Component Assembly

The Component Model has two binary formats. Wasm Core and the Component
format share the `\0asm` preamble and differ in the four-byte version word
after it. Two function-like macros cover both:

- A core-module macro accepts a string literal with the WAT of a
  `(module …)` and returns a `&'static [u8]` with the assembled binary.
- A component macro accepts a string literal with the WAT of a
  `(component …)` and returns a `&'static [u8]` with the assembled binary.

Both macros use the upstream [`wat`] assembler. Two macros instead of one
overloaded macro is a readability choice. The macro name documents the intent
of the binary at the call site.

The macros validate the assembled bytes against the binary header before they
emit code. If the version word does not match the declared kind, the compile
error names the expected kind, the actual kind, and the other macro. A WAT
parse error is a compile error at the macro call site, so the assembler's
line and column reach the editor of the contributor.

The macros run at compile time. The assembled bytes are ordinary byte-array
literals, so the constants work in `const` context and cost a slice reference
at run time.

## Errors at the Source

Every misuse of the macros is a compile error at the invocation:

- A syntax error in the WAT. The assembler error is kept verbatim.
- A binary-kind mismatch between the macro and the assembled output.
- A synchronous function marked with the cross-target attribute.
- An argument passed to the cross-target attribute.

Contributors get the same feedback loop they expect from the rest of the
toolchain. The test suite does not grow fixtures whose correctness depends on
review conventions.

## User Stories

A contributor writes a new test and wants it to run on both targets.

> The contributor writes `#[wcmp_macros::test] async fn …`. `test:native:debug`
> runs it under `tokio`. `test:web:debug` compiles it to
> `wasm32-unknown-unknown`, bundles it with `wasm-bindgen-test`, and runs it
> in headless Chrome.

A contributor writes a small component for a focused test and wants the
source next to the assertions.

> The contributor writes the WAT inside `component!(...)`, binds it to a
> `const`, and passes the constant to `Component::new`. There is no fixture
> directory, build script, or `.wasm` blob in the repository.

A contributor catches a mistake and wants the compiler to report it.

> The contributor pastes a `(module …)` into `component!(...)`. The compiler
> reports that the WAT assembled to a core module and points at `wasm!`.

A maintainer reviews a test and wants to read it as one artifact.

> The maintainer opens the file. The attribute, the inline component, and the
> assertions are visible together.

## References

- [PDD001], the development environment.
- [PDD002], the ecosystem foundation.
- [WAT], the WebAssembly Text Format.
- [`wat`], the assembler the macros invoke at compile time.
- [wasm-bindgen-test], the browser test harness.
- [tokio], the native asynchronous runtime.
- [Dialog DB `#[test]` macro], the pattern that inspired the attribute.

[PDD001]: ./PDD001%20Development%20Environment.md
[PDD002]: ./PDD002%20Ecosystem%20Foundation.md
[WAT]: https://webassembly.github.io/spec/core/text/index.html
[`wat`]: https://docs.rs/wat
[wasm-bindgen-test]: https://rustwasm.github.io/docs/wasm-bindgen/wasm-bindgen-test/index.html
[tokio]: https://tokio.rs
[Dialog DB `#[test]` macro]: <https://github.com/dialog-db/dialog-db/blob/00c7bc5fa8ea187da7abda27c2a0a8edbd8c05ed/rust/dialog-common/src/lib.rs#L132>
