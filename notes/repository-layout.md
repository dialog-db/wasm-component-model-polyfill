# Repository layout

| Path                    | Contents                                                                              |
| ----------------------- | ------------------------------------------------------------------------------------- |
| `rust/wcmp/`            | The library crate, its baseline tests, and the conformance harness and corpora.       |
| `rust/wcmp-macros/`     | Procedural macros: a cross-target `#[test]`, `#[bench]`, `wasm!`, `component!`.       |
| `rust/wcmp-smoke/`      | The end-to-end smoke test, one host program for both targets.                         |
| `rust/wcmp-bench/`      | The benchmark suite, one definition measured on both targets.                         |
| `rust/wcmp-wasm-core*/` | The runtime layer: its trait, one backend per engine, and the suites that prove them. |
| `project/design/`       | The Project Design Documents (PDDs), one per design decision.                         |
| `project/kanban/`       | The project board.                                                                    |

## Design documents

Every design decision is recorded as a PDD under `project/design/` before it is
built. Start with `PDD000`, the product overview, `PDD002`, the ecosystem
foundation, and `PDD003`, the compatibility outlook that maps the Component
Model onto the polyfill feature by feature. `PDD018` through `PDD021` design the
concurrency runtime, the callback export, subtasks and the asynchronous import,
and streams and futures. `PDD022` designs the suspend providers, the stackful
export, and the thread built-ins, and `PDD023` designs cancellation, error
contexts, and the poisoned store. Both are built.
