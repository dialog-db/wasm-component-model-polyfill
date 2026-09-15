# Conformance Suite

The tests of [PDD004] are hand-written components next to their assertions. They
are the right tool for the public API. They are the wrong tool for Canonical ABI
conformance, because the space of value shapes, encodings, and edge cases is
large and already enumerated upstream. Two `.wast` suites exist: the [Component
Model test corpus] and the [Wasmtime component tests]. This PDD makes both
suites part of the polyfill's test regime.

## Goals

- The polyfill runs a `.wast` script whose directives use the Component Model
  text format, on every supported target, through the same menu commands as the
  rest of the suite.
- Both upstream corpora run against the polyfill. An expected-failure list
  records each case that the polyfill does not pass yet, with a reason.
- A case that passes and is not in the list, or a case in the list that fails
  for a different reason, fails the run. Stale expectations do not accumulate.
- The polyfill reports a trap with the message Wasmtime uses, so that
  `assert_trap` directives match exactly.
- A small set of real guest components, built with `wit-bindgen` and composed
  with `wac`, lives in the repository with its WIT and runs under the same
  harness.

## Non-goals

- The `async` directories of either corpus, and the `gc`, thread, and memory64
  directories of the Wasmtime corpus. Each directory joins the run when the
  feature it exercises is in scope.
- A `.wast` interpreter for Wasm Core. The harness handles component-level
  directives and delegates core modules to the runtime layer.
- Replacement of the [PDD004] tests. The two regimes coexist. [PDD004] tests the
  public API. The conformance suite tests the Canonical ABI.

## The Harness

A `.wast` script is a sequence of directives: define a component, register it
under a name, invoke an export with literal arguments, and assert a result, a
trap, an invalid binary, or an unlinkable component. The harness parses the
script with the upstream [`wast`] crate, which understands the component text
format. It maps each directive onto the public API of the polyfill:

- A component definition becomes `Component::new`.
- A `register` directive makes the component's exports available as imports of
  later components through a `Linker`.
- An `invoke` directive becomes an export call with literal `Val` arguments.
- An `assert_return` compares the lifted result with the expected literal.
- An `assert_trap` compares the error message with the expected message.
- An `assert_invalid` and an `assert_unlinkable` expect `Component::new` or
  `Linker::instantiate` to fail.

The harness is a test in the workspace. It carries the cross-target attribute of
[PDD004], so the corpora run natively and in the browser. The corpora are
vendored into the repository at a recorded upstream commit, so that a run is
reproducible and an upstream change is a deliberate update.

## Expected Failures

An expected-failure file lists each failing case by corpus, file, and directive
index, with a category and one line of reason. The Component Model corpus keeps
its own list in `nyi.txt`. The polyfill keeps a list in the same shape. A run
compares its results with the list in both directions. A new pass removes its
entry. A new failure adds one. Either change is a reviewed edit.

The category names the cause of the failure. The vocabulary is fixed: a deferred
feature, a limit of the runtime layer, a validation gap, a trap message that
differs from Wasmtime, a wrong result, or a cascade from an earlier failure in
the same file. The harness rejects an entry without a category.

## Progress

The run prints a summary per corpus: the number of directives, the number that
pass, the pass percentage, and the expected failures per category. The summary
is the progress metric of the polyfill against the specification. The run also
writes the summary as JSON, so that tooling can track the metric over time. The
menu has a command that prints only the summary.

## Real Guests

A fixtures directory holds a few components built from real toolchains: a Rust
guest from `wit-bindgen`, a guest that exports a resource, and a composition of
two guests from `wac`. Each fixture carries its WIT and the command that built
it. The fixtures are binary files checked into the repository, because a build
from source needs toolchains the flake does not provide. A fixture is updated
when its toolchain is updated, and the update records the new version.

## User Stories

A contributor implements a Canonical ABI rule and wants evidence that the rule
holds across every shape upstream enumerates.

> The contributor runs the conformance command. The run reports which directives
> pass and which are expected failures, and prints the summary per corpus. They
> remove the entries their change fixed and watch the pass percentage rise.

A maintainer updates the vendored corpora and wants to see what changed
upstream.

> The maintainer bumps the recorded commit. The run reports new directives. The
> maintainer triages each into passing, expected failure with a reason, or a
> defect to fix.

A developer wants to trust that a component from a real toolchain runs.

> The developer reads the fixtures directory and sees a `wit-bindgen` guest and
> a `wac` composition run on every target in CI.

## Test Cases

A `.wast` file runs through the harness. Every vendored file becomes one test,
and each directive maps onto the public API with the result the directive
asserts.

The expected-failure list stays current. A run fails when a listed directive
passes, when an unlisted directive fails, or when a list entry has no category.

The progress summary is complete. The run prints one row per corpus with the
directive count, the pass count, the pass percentage, and the expected failures
per category, and writes the same summary as JSON.

Real guests run under the harness. Each checked-in fixture loads, instantiates,
and answers its assertions on the native target, and the fixtures regenerate
byte-for-byte from their sources and recorded commands.

## References

- [PDD000], the product overview.
- [PDD001], the development environment and menu.
- [PDD003], the compatibility outlook, which names both corpora as sources of
  truth.
- [PDD004], the test macros.
- [PDD006], component parsing.
- [PDD007], linking and instantiation.
- [PDD015], component composition.
- The [Component Model test corpus] and its `nyi.txt`.
- The [Wasmtime component tests].
- [`wast`], the crate that parses the script format.
- [`wit-bindgen`] and [`wac`], the toolchains that build the fixtures.

[PDD000]: ./PDD000%20Wasm%20Component%20Model%20Polyfill.md
[PDD001]: ./PDD001%20Development%20Environment.md
[PDD003]: ./PDD003%20Compatibility%20Outlook.md
[PDD004]: ./PDD004%20Test%20Macros.md
[PDD006]: ./PDD006%20Component%20Parsing.md
[PDD007]: ./PDD007%20Linking%20and%20Instantiation.md
[PDD015]: ./PDD015%20Component%20Composition.md
[Component Model test corpus]:
  https://github.com/WebAssembly/component-model/tree/main/test
[Wasmtime component tests]:
  https://github.com/bytecodealliance/wasmtime/tree/main/tests/misc_testsuite/component-model
[`wast`]: https://docs.rs/wast
[`wit-bindgen`]: https://github.com/bytecodealliance/wit-bindgen
[`wac`]: https://github.com/bytecodealliance/wac
