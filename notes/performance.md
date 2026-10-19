# Performance

The numbers below come from the benchmark suite (`rust/wcmp-bench`). Absolute
times depend on the machine. Compare a benchmark with itself before and after a
change, on one machine (`rust/wcmp-bench/README.md`). The baseline `afc71435f`
is the last commit before every browser call went through JSPI.

## Host suspension in the browser

When every browser export call became a JSPI resumable call (card `7eb780`),
each call cost more: from about 1 µs for `u32-call` to about 42 µs for
`list-u32/4096`. Card `721eac` recorded these figures, with the benchmark names
as the card gives them:

| Benchmark          | Before (µs) | After the change (µs) |
| ------------------ | ----------- | --------------------- |
| `u32-call`         | 3.333       | 4.381                 |
| `string/64`        | 4.158       | 6.800                 |
| `list-u8-typed/64` | 3.889       | 6.333                 |
| `list-u32/4096`    | 78.0        | 120.5                 |

Card `721eac` then made a call that never suspends end on its first poll, with
no JavaScript closure, no promise handler, and no microtask. Its reviewer
measured every string and list benchmark at 0.875 to 0.997 times the baseline;
`string/64`, for example, went from 4.32 to 3.78 µs. Card `f87de5` closed the
remaining gaps. Its reviewer's paired ratios against the baseline were 0.89 for
`u32-call-typed`, 0.90 for `composition-call`, 0.91 for `resource-handle`, 0.97
for `host-calls-in-flight/8`, and 0.99 for `host-calls-in-flight/64`. Native
ratios were 0.91 to 1.00. Card `bf9a7f` records one older gap:
`list-u8-roundtrip/65536` is about 1 to 2% slower than the baseline.

A call that does suspend still pays for a promise and a microtask at each
resumption (crate docs of `rust/wcmp-wasm-core-web`).

## Floats in the browser

The JavaScript API carries a float as a `Number`, which can change the bits of a
NaN. So the backend calls each export whose type holds a float through a
carrier: a small generated module that passes an `f64` as the `i64` of its bits,
which JavaScript receives as a `BigInt`
(`rust/wcmp-wasm-core-web/src/carrier.rs`). The `f64-call` benchmarks measure
the cost against the `u32` floor of the same shape.

Two `bench web target-sample-ms=50` runs of 2026-10-13 gave these times, in µs:

| Benchmark        | Run 1 | Run 2 |
| ---------------- | ----- | ----- |
| `u32-call`       | 2.04  | 2.06  |
| `f64-call`       | 2.22  | 2.18  |
| `u32-call-typed` | 1.99  | 1.96  |
| `f64-call-typed` | 2.13  | 2.12  |

A float call costs about 0.15 µs more in the browser, 6 to 9%. Natively the two
are the same: `bench native` put `u32-call` at 1.36 µs and `f64-call` at 1.35 µs
(card `276004`).
