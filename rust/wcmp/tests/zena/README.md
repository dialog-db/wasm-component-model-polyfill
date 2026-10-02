# Zena compatibility

The scenarios under `scenarios/` compile real [Zena] programs with Zena's own
toolchain, at the revision that the flake lock pins. Each component runs under
Wasmtime, under the polyfill in the browser, and under the polyfill natively.
Each run stops at a stage, from `compile` to `pass`.

`record.txt` beside this file is the source of truth. It holds the stage of
every scenario for every subject, and a run fails when any stage differs from
it. The report below is a dated copy for a person to read. When the two
disagree, the record is right.

## Commands

- `tests zena` runs the three subjects, holds them to the record, and prints the
  compatibility report.
- `tests zena regenerate` writes the record again from all three subjects.
  `tests zena regenerate --dry-run` only prints the difference.

`tests all` runs the scenarios as one of its lanes.

## Report

The report of `tests zena` on 2026-10-02:

```text
zena at 1bbe472f4c34d3faf12c74f9876afcfe541c4ac3

- async-sleep
  - Browser: pass
  - Native: pass
  - Wasmi: parse (component sleeper: unsupported component feature: gc)
  - Wasmtime: pass
- classes-and-arrays
  - Browser: pass
  - Native: pass
  - Wasmi: parse (component shapes: unsupported component feature: gc)
  - Wasmtime: pass
- compiler-custom-world
  - Browser: pass
  - Native: pass
  - Wasmi: parse (component compiler: unsupported component feature: gc)
  - Wasmtime: pass
- compiler-http-handler
  - Browser: pass
  - Native: pass
  - Wasmi: parse (component compiler: unsupported component feature: gc)
  - Wasmtime: pass
- compiler-scalar-export
  - Browser: pass
  - Native: pass
  - Wasmi: parse (component compiler: unsupported component feature: gc)
  - Wasmtime: pass
- compiler-type-error
  - Browser: pass
  - Native: pass
  - Wasmi: parse (component compiler: unsupported component feature: gc)
  - Wasmtime: pass
- console-output
  - Browser: pass
  - Native: pass
  - Wasmi: parse (component printer: unsupported component feature: gc)
  - Wasmtime: pass
- custom-world
  - Browser: pass
  - Native: pass
  - Wasmi: parse (component relay: unsupported component feature: gc)
  - Wasmtime: pass
- exceptions
  - Browser: pass
  - Native: pass
  - Wasmi: parse (component halver: unsupported component feature: gc)
  - Wasmtime: pass
- rust-imports-zena-composition
  - Browser: pass
  - Native: pass
  - Wasmi: parse (component importer: unsupported component feature: gc)
  - Wasmtime: pass
- rust-imports-zena-run-time
  - Browser: pass
  - Native: pass
  - Wasmi: parse (component exporter: unsupported component feature: gc)
  - Wasmtime: pass
- rust-link-run-time
  - Browser: pass
  - Native: pass
  - Wasmi: pass
  - Wasmtime: pass
- scalar-export
  - Browser: pass
  - Native: pass
  - Wasmi: pass
  - Wasmtime: pass
- string-roundtrip
  - Browser: pass
  - Native: pass
  - Wasmi: parse (component strings: unsupported component feature: gc)
  - Wasmtime: pass
- zena-imports-rust-composition
  - Browser: pass
  - Native: pass
  - Wasmi: parse (component importer: unsupported component feature: gc)
  - Wasmtime: pass
- zena-imports-rust-run-time
  - Browser: pass
  - Native: pass
  - Wasmi: parse (component importer: unsupported component feature: gc)
  - Wasmtime: pass
- zena-link-composition
  - Browser: pass
  - Native: pass
  - Wasmi: parse (component importer: unsupported component feature: gc)
  - Wasmtime: pass
- zena-link-run-time
  - Browser: pass
  - Native: pass
  - Wasmi: parse (component exporter: unsupported component feature: gc)
  - Wasmtime: pass

Passes: Browser 18/18, Native 18/18, Wasmi 2/18, Wasmtime 18/18
```

## Compiler scenarios

The four scenarios whose names start with `compiler-` run Zena's own compiler as
a component on every subject. The build makes the compiler component from the
pinned toolchain's source tree and the entry module in `zena/compiler/` at the
root of this repository. A scenario that holds a `compiler.txt` gets that
component as its program `compiler`. Its expectations hold the Zena source of
each compile as a string argument.

- `compiler-scalar-export` compiles the program of `scalar-export`, then
  instantiates the result and calls it.
- `compiler-type-error` compiles a program with a type error. The call returns
  `err`, and the text names the file and the line.
- `compiler-custom-world` compiles the program of `custom-world` against its
  declared world, then instantiates the result with the test host import and
  calls it.
- `compiler-http-handler` compiles a program that exports
  `wasi:http/handler@0.3.0`. The test host has no `wasi:http`, so the scenario
  does not instantiate the result.

The compiler's `read-source` import reads every file other than the entry
module. The test host answers it from the toolchain's source bundle, which holds
Zena's standard library. A path the bundle lacks answers none.

An expectation `-> component <name>` asks for the bytes of a component. The
runner parses, links, and instantiates them under `<name>`, and later calls name
that component. The stages apply as they do to a scenario's own components. The
Wasmtime run records the SHA-256 digest of the bytes, and a polyfill subject
passes a compile only when it returns the same bytes. `-> component` with no
name checks the bytes and loads nothing. `-> err containing "<text>"` asks for
an `err` whose text contains `<text>`.

These scenarios are expensive. The compiler is about 2.3 MB, and natively one
compile can take several seconds. A failure in one can have many causes, so
these scenarios do not locate a small fault. The other scenarios still do that.
Wasmi has no GC, so the Wasmi subject stops each of them at `parse`.

## Moving the pin

1. Run `nix flake update zena`.
2. Run `tests zena regenerate`.
3. Read the difference in `record.txt`.
4. Commit the flake lock and the record together, in one commit.

A person updates the report above at each move of the pin, with the date of the
run.

## The first predictions

Before the first run, a reading of Zena's output and of the runtime layer made
four predictions about where the components would stop. Two records settle them.
The first is the record at `129e7e80c`, on the earlier runtime layer (Browser
2/14, Native 3/14, Wasmtime 14/14). The second is the record at `faf3e8510`,
after the switch to the new runtime layer, where every scenario passes on all
three subjects. The pin is `b2237f7e` in both, so the components are the same.

In the earlier record every stop in the browser and natively was at `parse`, not
at `instantiate`. The earlier layer refused a core module when `Component::new`
compiled it. The error text reads "instantiation error" because the polyfill
wrapped that refusal in its instantiation error.

**A tag export stops the component at `instantiate` (scenario 4,
`exceptions`).** Refuted. Natively, the tag stopped the component, but at
`parse`:

> component halver: instantiation error: the runtime substrate failed to
> instantiate the component: tags are not supported in the wasm_runtime_layer

In the browser the component stopped at `parse` on its GC-typed global, before
any refusal of the tag:

> component halver: instantiation error: the runtime substrate failed to
> instantiate the component: reference type (ref null (module 4)) is not
> supported in the wasm_runtime_layer

After the switch, scenario 4 passes in the browser and natively.

**A global with a GC reference type stops the component at `instantiate` in the
browser (scenario 3, `classes-and-arrays`).** Refuted. The global stopped the
component in the browser, but at `parse`:

> component shapes: instantiation error: the runtime substrate failed to
> instantiate the component: reference type (ref null (module 1)) is not
> supported in the wasm_runtime_layer

Natively, scenario 3 passed in both records. After the switch it passes in the
browser too.

**A string export carries the tag and its global (scenario 2,
`string-roundtrip`).** Confirmed. In the earlier record, scenario 2 stopped at
`parse` on the global in the browser and on the tag natively:

> component strings: instantiation error: the runtime substrate failed to
> instantiate the component: reference type (ref null (module 8)) is not
> supported in the wasm_runtime_layer

> component strings: instantiation error: the runtime substrate failed to
> instantiate the component: tags are not supported in the wasm_runtime_layer

Scenario 1, `scalar-export`, has no string and no exception, and passed on both.
So the string export brought the tag and the global. Every other scenario with a
Zena component that passes a string stopped with the same two refusals. After
the switch, scenario 2 passes in the browser and natively.

**A GC type never reaches the conversion of export types (scenarios 2 and 3).**
Confirmed. No error text in either record names an export type. In the earlier
record, scenario 3 passed natively, so every export type of a program that uses
classes and arrays converted. The other stops of scenarios 2 and 3 came earlier,
at the core module. After the switch, both scenarios pass in the browser and
natively, so every export type of both converted on both targets.

[Zena]: https://zena-lang.dev/
