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

The report of `tests zena` on 2026-09-30:

```text
zena at b2237f7e65847eda43ef1f4094eea77fe225ce0d

- async-sleep
  - Browser: pass
  - Native: pass
  - Wasmtime: pass
- classes-and-arrays
  - Browser: pass
  - Native: pass
  - Wasmtime: pass
- console-output
  - Browser: pass
  - Native: pass
  - Wasmtime: pass
- custom-world
  - Browser: pass
  - Native: pass
  - Wasmtime: pass
- exceptions
  - Browser: pass
  - Native: pass
  - Wasmtime: pass
- rust-imports-zena-composition
  - Browser: pass
  - Native: pass
  - Wasmtime: pass
- rust-imports-zena-run-time
  - Browser: pass
  - Native: pass
  - Wasmtime: pass
- rust-link-run-time
  - Browser: pass
  - Native: pass
  - Wasmtime: pass
- scalar-export
  - Browser: pass
  - Native: pass
  - Wasmtime: pass
- string-roundtrip
  - Browser: pass
  - Native: pass
  - Wasmtime: pass
- zena-imports-rust-composition
  - Browser: pass
  - Native: pass
  - Wasmtime: pass
- zena-imports-rust-run-time
  - Browser: pass
  - Native: pass
  - Wasmtime: pass
- zena-link-composition
  - Browser: pass
  - Native: pass
  - Wasmtime: pass
- zena-link-run-time
  - Browser: pass
  - Native: pass
  - Wasmtime: pass

Passes: Browser 14/14, Native 14/14, Wasmtime 14/14
```

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
