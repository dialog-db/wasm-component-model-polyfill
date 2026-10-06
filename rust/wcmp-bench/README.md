# The benchmark suite

One benchmark definition, measured on both targets.

The polyfill runs on a native engine and in a browser, and the same code path
can cost wildly different amounts on the two. A value crosses the canonical ABI
through runtime-layer memory reads and writes, each of which in a browser is a
JavaScript boundary crossing, and the untyped `Val` path allocates per value.
This suite exists so that those costs are numbers rather than suspicions, and so
that a change can be read on both targets from one definition.

## Running it

```
bench native
bench wasmi
bench web
```

Each command builds the suite as a Nix derivation and then runs it. The
measurement itself is deliberately not a derivation: a derivation's output is
cached, and a cached benchmark result is a stale one. Nix builds the binary; the
menu command measures with it, every time.

`bench native` runs the suite as a plain binary over the Wasmtime backend of the
runtime layer, and `bench wasmi` runs the same binary over the Wasmi backend: it
sets `WCMP_BENCH_BACKEND=wasmi`, so it builds nothing more. `bench web` serves
the same suite, compiled to `wasm32-unknown-unknown` and bundled by
`wasm-bindgen`, and drives it in headless Chromium through the same WebDriver
plumbing the browser tests use.

Each writes a JSON report under the cargo target directory, as the conformance
summary does:

```
$CARGO_TARGET_DIR/bench/native.json
$CARGO_TARGET_DIR/bench/wasmi.json
$CARGO_TARGET_DIR/bench/web.json
```

Each also prints a table. Run controls follow the leaf as `key=value` words, and
mean the same thing on either target:

| control            | default | what it does                                  |
| ------------------ | ------- | --------------------------------------------- |
| `warmup`           | 16      | iterations run before anything is measured    |
| `samples`          | 25      | timed batches per benchmark                   |
| `target-sample-ms` | 5.0     | how long one timed batch should last          |
| `max-batch`        | 100000  | the ceiling on a batch's iterations           |
| `provider`         | on      | `off` turns the engine's suspend provider off |

```
bench native samples=50 warmup=4
bench web samples=5
bench web provider=off
```

`bench web` also reads `WCMP_BENCH_BUDGET_SECONDS` (900 by default) for how long
the whole browser run may take before the driver gives up.

## Writing a benchmark

A benchmark is an `async fn` under `src/suite.rs` carrying the
`#[wcmp_macros::bench]` attribute. It sets its guest up once, then loops on
`run.iterate()`:

```rust
#[wcmp_macros::bench(
    guest = "the `guest` corpus fixture: `double: func(x: u32) -> u32`",
    payload = "one u32 in and one u32 out, as `Val`"
)]
async fn u32_call(run: &mut Run) -> Result<()> {
    let (_engine, mut store, instance) = instantiate(guests::GUEST).await?;
    let double = export(&instance, "double")?;
    let arguments = [Val::U32(21)];
    while run.iterate() {
        double.call(&mut store, &arguments).await?;
    }
    Ok(())
}
```

Nothing in a body is per-target. `run.iterate()` owns the clock, the warm-up,
and the batching, and it is the only place the two targets differ: natively it
reads the monotonic system clock, in a browser `performance.now()`.

`guest` and `payload` are required, and both reach the report, so that every
number says which guest produced it and what moved across the boundary.
`cases = [...]` repeats one definition over several payload sizes or several
named guests. A body states the size of what it moves with `run.moves_bytes` or
`run.moves_elements`, which is where the report's throughput column comes from.

## What is measured

| benchmark                   | what it is for                                                 |
| --------------------------- | -------------------------------------------------------------- |
| `u32-call`                  | the floor: a call with no memory traffic under it              |
| `u32-call-typed`            | the same call without the untyped `Val` path's allocation      |
| `string-roundtrip/N`        | a string lowered into guest memory and lifted back             |
| `string-roundtrip-typed/N`  | the same string through `TypedFunc` rather than `Val`          |
| `list-u8-roundtrip/N`       | the bytes of a string, as a list of `u8` values                |
| `list-u8-roundtrip-typed/N` | the same bytes as a `Vec<u8>` through `TypedFunc`              |
| `list-u32-roundtrip/N`      | a list of numbers, copied straight to and from its bytes       |
| `list-record-roundtrip/N`   | the same, with a two-field record per element                  |
| `resource-handle`           | one owned handle minted, passed, handed back, and dropped      |
| `composition-call`          | a call through the adapter `wac plug` wrote between two guests |
| `host-calls-in-flight/N`    | N host calls pending at once, completing one per turn          |
| `yields`                    | one callback call that yields 16 times through the driver      |
| `component-new/<fixture>`   | parsing and translating a component binary                     |

Every benchmark names its guest and its payload in the report's second block and
in the JSON, so the table above is a summary, not the record.

## How to read the numbers

A benchmark's iterations are not fixed in advance. The warm-up is timed as a
whole, and its cost per iteration proposes a batch size; one untimed calibration
batch then grows that size until a batch lasts about `target-sample-ms`, which
is what keeps a browser's clamped clock from being the thing measured. The
report quotes:

- **iterations** — every iteration the body ran, including the warm-up and the
  calibration batches.
- **batch** — the iterations one timed sample covers. A batch far above 1 means
  one iteration is far below the clock's resolution; that is expected in a
  browser, where Chromium rounds `performance.now()` to 100 microseconds unless
  the page is cross-origin isolated.
- **median (us)** — the median sample's microseconds per iteration. The median,
  not the mean: a browser interleaves work the benchmark did not ask for, and
  one long sample should not move the number a reader compares.
- **spread %** — how far the tenth and ninetieth percentile samples lie apart,
  as a percentage of the median. It is the run's own noise. A change smaller
  than the spread has not been measured; run more samples or move to a quieter
  machine before believing it.
- **throughput** — derived from the median and from the bytes or elements the
  benchmark said it moves.

The JSON carries the same fields per benchmark, plus `min_ns`, `p10_ns`,
`p90_ns`, the per-iteration payload size, and the error when a benchmark failed.
Its shape is identical on both targets, so two reports can be diffed or joined
on `name`. Its `target` and `backend` name where it was measured: `native` over
`wasmtime` or `wasmi`, or `wasm32-unknown-unknown` over `web`.

## What does not compare across targets

The absolute times do not. A native engine compiles ahead of the call and a
browser goes through a JavaScript boundary on every memory access; one number
being ten or a hundred times the other says nothing on its own about either.

What does compare:

- **The same benchmark on one target, before and after a change.** This is the
  measurement the suite is for.
- **The shape across benchmarks on one target.** That a `list<record>` costs
  more per element than a `list<u32>` of the same length, or that a call through
  a composition costs more than a call into one guest, is a fact about the
  polyfill on that target, and it holds whatever the machine.
- **The ratio between the targets, watched over time.** Any single ratio is a
  property of the machine, the browser build, and the day. A ratio that moves
  after a change is a signal about the change.

Beyond that, a few specifics:

- The two runs use different clocks. The native clock is the monotonic system
  clock; the browser's is clamped, which is why a browser's batches are larger.
  A batch of 1 next to a batch of 2000 is not a discrepancy, it is the clock.
- The browser figure includes the event loop. A call that awaits a promise pays
  for a turn of the event loop, which a reader should count as part of what the
  polyfill costs in a browser rather than discount as overhead.
- `bench native` builds with the `release` profile and `bench web` with the same
  profile through `wasm-bindgen`; neither number describes a debug build, which
  is slower by a wide and uninteresting margin.
- A machine running anything else is not a benchmark machine. Inside a sandbox
  VM, next to a browser, or during a Nix build, the spread column will say so.
