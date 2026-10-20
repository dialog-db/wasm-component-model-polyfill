# Conformance

The test suite runs two `.wast` corpora on both targets: the [Component Model
test corpus] and the [Wasmtime component tests]. Every directive the polyfill
does not pass is listed, with a reason, in
`rust/wcmp/tests/corpus/expected-failures.txt` and two overlays beside it, one
for the browser and one for running without a suspend provider. The harness
fails when a listed directive starts to pass or an unlisted one fails, so the
lists stay current.

The table gives directives passed, with the pass percentage, from the
`tests all` run of 2026-10-20. The Baseline section of
`rust/wcmp/tests/corpus/README.md` gives the same figures by failure category,
and `tests conformance` prints the current numbers.

| Corpus           | Directives | Native, stack switching | Browser, JSPI | Native, no provider | Browser, no provider | Wasmi       |
| ---------------- | ---------- | ----------------------- | ------------- | ------------------- | -------------------- | ----------- |
| `cm`             | 1130       | 1057 (93.5)             | 1056 (93.5)   | 1057 (93.5)         | 1056 (93.5)          | 1042 (92.2) |
| `cm/async`       | 613        | 461 (75.2)              | 461 (75.2)    | 423 (69.0)          | 423 (69.0)           | 461 (75.2)  |
| `fixtures`       | 71         | 65 (91.5)               | 65 (91.5)     | 65 (91.5)           | 65 (91.5)            | 65 (91.5)   |
| `wasmtime`       | 481        | 476 (99.0)              | 473 (98.3)    | 476 (99.0)          | 473 (98.3)           | 445 (92.5)  |
| `wasmtime/async` | 425        | 425 (100.0)             | 425 (100.0)   | 400 (94.1)          | 400 (94.1)           | 410 (96.5)  |
| total            | 2720       | 2484 (91.3)             | 2480 (91.2)   | 2421 (89.0)         | 2417 (88.9)          | 2423 (89.1) |

- Three files of the Component Model corpus use `stream.forward`, which neither
  the polyfill nor Wasmtime 50 implements. They hold 213 of the 236 directives
  that fail natively. Wasmtime's own runner lists the same three files as
  failing.
- With a suspend provider, `wasmtime/async` passes every directive on both
  targets, and `cm/async` passes every directive outside those files.
- The browser fails four more directives than native. One is V8's wording of a
  validation error. One needs memory past 4 GiB, and its trap poisons the store
  for the file's next two (`rust/wcmp/tests/corpus/expected-failures.web.txt`).
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
