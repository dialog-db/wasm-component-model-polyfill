# Conformance

The test suite runs two `.wast` corpora on both targets: the [Component Model
test corpus] and the [Wasmtime component tests]. Every directive the polyfill
does not pass is listed, with a reason, in
`rust/wcmp/tests/corpus/expected-failures.txt` and two overlays beside it, one
for the browser and one for running without a suspend provider. The harness
fails when a listed directive starts to pass or an unlisted one fails, so the
lists stay current.

The table gives directives passed, with the pass percentage, from the
`tests all` run of 2026-10-13. The Baseline section of
`rust/wcmp/tests/corpus/README.md` gives the same figures by failure category,
and `tests conformance` prints the current numbers.

| Corpus           | Directives | Native, stack switching | Browser, JSPI | Native, no provider | Browser, no provider | Wasmi       |
| ---------------- | ---------- | ----------------------- | ------------- | ------------------- | -------------------- | ----------- |
| `cm`             | 1126       | 1106 (98.2)             | 1105 (98.1)   | 1106 (98.2)         | 1105 (98.1)          | 1092 (97.0) |
| `cm/async`       | 393        | 393 (100.0)             | 393 (100.0)   | 361 (91.9)          | 361 (91.9)           | 393 (100.0) |
| `fixtures`       | 71         | 65 (91.5)               | 65 (91.5)     | 65 (91.5)           | 65 (91.5)            | 65 (91.5)   |
| `wasmtime`       | 469        | 464 (98.9)              | 460 (98.1)    | 464 (98.9)          | 460 (98.1)           | 433 (92.3)  |
| `wasmtime/async` | 387        | 387 (100.0)             | 387 (100.0)   | 363 (93.8)          | 363 (93.8)           | 372 (96.1)  |
| total            | 2446       | 2415 (98.7)             | 2410 (98.5)   | 2359 (96.4)         | 2354 (96.2)          | 2355 (96.3) |

- With a suspend provider, the `async` corpora pass every directive on both
  targets.
- The browser fails five more directives than native. One is V8's wording of a
  validation error. One needs memory past 4 GiB, and its trap poisons the store
  for the file's next two. One is the trap that loses its reason under
  [host suspension](suspend-providers.md#limits-of-host-suspension)
  (`rust/wcmp/tests/corpus/expected-failures.web.txt`).
- Without a provider, the extra failures are blocks that only a stack switch can
  serve, plus later directives in the same file that then meet a poisoned store.
- On Wasmi, the extra failures are components that need a capability Wasmi
  lacks, and the directives that fail after them.
- The two WASI 0.3 HTTP fixtures stop at link, because the harness provides no
  `wasi:http/types` host.

[Backends](backends.md#fidelity) sets these figures beside the WebAssembly
specification tests and the Zena scenarios.

[Component Model test corpus]:
  https://github.com/WebAssembly/component-model/tree/main/test
[Wasmtime component tests]:
  https://github.com/bytecodealliance/wasmtime/tree/main/tests/misc_testsuite/component-model
