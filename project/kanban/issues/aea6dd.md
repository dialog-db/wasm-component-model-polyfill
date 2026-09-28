---
id: aea6dd
title: Two Zena components link at run time
type: feature
blocked_by: [568112]
labels: [PDD024, zena]
created: 2026-09-27T16:53:37Z
---

## What to build
Scenarios with more than one component, linked at run time, starting with two Zena components.

- A scenario can hold several Zena programs, one WIT world that is the contract between them (each compiles with `--wit` and `--world`), and the wiring: which component's exports satisfy which component's imports, and whether the link is made at run time or by composition. Expectations entries name the component they call.
- At run time, the runner instantiates the exporting component first and gives that instance's exports to the importing component through the polyfill's `Linker`. The Wasmtime run does the same through Wasmtime's `Linker`.
- Scenario 8: Zena imports from Zena at run time, passing a string across the link.
- Regenerate and commit the record lines.

Notes from the review of 264c89 (landed as `b882bd371`):
- The polyfill runner instantiates each component on its own (`tests/zena/runner.rs:81-86`). Run-time-linked scenarios need runner code on both the polyfill and the Wasmtime side.
- The polyfill links and instantiates in one call, so the runner tells `link` from `instantiate` by error variant. With several components, the stage order can differ from the Wasmtime run, which parses all, links all, then instantiates all.

## Acceptance criteria
- [ ] Scenario 8 runs under all three subjects, linked at run time.
- [ ] The record holds lines for scenario 8, and `tests zena` passes the gate.
- [ ] `tests native debug`, `tests web debug`, and `lint` pass.


## Review notes
### Round 1 (`review-aea6dd-f65e025b`, tip `d063e00bb`): needs-changes

> VERDICT: needs-changes. The gates are green and the Wasmtime side is solid. My objection is test quality: the polyfill link code has no test that matches what Zena emits and no test that checks the argument crosses the link. Both polyfill subjects stop at parse on scenario 8, so the WAT tests are the only evidence for web and native.
> AC1 met in substance on wasmtime only (wasmtime_run.rs:135-162, forward 183-266). The web and native subjects stop at parse on the exporter (tags, and ref null (module 8) on the browser), before any link. That is honest: the PDD records stages rather than requiring a pass. AC2 met (record.txt:34-36). tests zena passes, and 'tests zena regenerate --dry-run' says the record is current. AC3 met.
> BLOCKING 1: the GREETER WAT test component (tests/zena.rs:665-700, and the same copy at wcmp-wasmtime wasmtime_run.rs:433-468) never reads its name argument. greet always writes 'hello' from offset 0. So it_links_async_functions_... (zena.rs:1029) passes even if the host function drops or corrupts the argument string. Only the result half of 'passing a string across the link' is tested on web and native.
> BLOCKING 2: the WAT tests do not match Zena's output. I ran wasm-tools print on the built zena-scenarios. Zena's importer uses 'canon lower (func 0) async', which goes through subtask and waitable-set. Both Zena components lift with 'async (callback ...)'. CALLER uses a sync canon lower and GREETER a sync lift (zena.rs:728-760). So the path Zena will actually take once tags work (an async-lowered import served by func_new_concurrent that calls a callback-lifted export through call_concurrent) is tested nowhere in the browser or natively. Fix: a greeter that uses its argument (e.g. 'hello, ' + name), plus a variant with an async lower and a callback lift.
> Implementor claims, checked in the wasmtime-49.0.0-rc.1 crate source: (a) a nested run_concurrent panics. check_recursive_run in concurrent.rs:6108-6114 panics with 'Recursive StoreContextMut::run_concurrent calls not supported', and Func::call_async goes through run_concurrent_trap_on_idle when concurrency support is on (func.rs:293-301). (b) A sync-typed import cannot take func_new_concurrent: typecheck_async in func/host.rs:577-595. Sync Func::call fails validate_sync_call on an async store (func.rs:259-266). (c) Polyfill: Func::call takes &mut Store (instance/func.rs:204), which a sync HostCall cannot reach. So the async-only restriction is real in both runtimes. It narrows PDD024, which puts no async condition on run-time links. The Rust-partner scenarios 10-13 will need async contracts too. That is the owner's call to record; I would not block on it.
> NON-BLOCKING: (3) The doc at runner.rs:86-88 and wasmtime_run.rs:74-76 says 'each function of the item the importer imports', but the code iterates the exporter's exports (runner.rs:169-204). (4) Stage divergence: the polyfill registers the exporter's FunctionType, so a type disagreement fails at link. Wasmtime's func_new_concurrent is dynamically typed, so the same disagreement surfaces at call. The runner.rs:81-84 divergence (exporter fails instantiate while importer fails link) is documented. Neither fails the gate, because each subject has its own record line, but either one misattributes the fault. (5) A wiring typo such as a wrong import name is recorded as a link stage on every subject rather than failing as a layout error, so the gate would accept a broken scenario. (6) wcmp-wasmtime Scenario::read's new wiring paths (Error::Wiring, and Layout for composition, scenario.rs:101-126) have no test and no wasmtime-check case. (7) Cancellation, an exporter trap and store drop are traced but untested. The host future holds an Arc of the instance list (polyfill) or looks the instance up in store data (wasmtime), with no store borrow held across an await; the lock is released before await (runner.rs:258-266). A trap ends the driver and fails the importer's call. A cancelled subtask drops the call_concurrent future, and the exporter's task runs on unobserved.
> Conventions: OK (one public type per module, no pub(crate), BDD names, structured Error variants, no dependency changes). wiring.txt is documented in the Wiring rustdoc (wiring.rs:10-27), the build.sh header and the scenario's own header: enough to write one.
> GATES at d063e00bb: 'tests all' exit 0 (native debug 1387/1387, native release passed, web debug 1371/1371, web release passed, native and web no-provider 169, tests zena passed; the new link tests passed in the native and both web lanes); 'tests zena' exit 0 (Browser parse, Native parse, Wasmtime pass); 'tests zena regenerate --dry-run' reports the record current; 'lint' exit 0 (all checks passed). MENU GAP: no menu command to disassemble built scenarios, so I used 'nix develop -c wasm-tools print' on the zena-scenarios store path. I also fetched the wasmtime 49 crate from crates.io into scratch to read its source. Nothing left running: all tests, lint and monitor jobs finished or expired; no code changed, no commits or pushes.

Owner design note: run-time links work only for async-typed functions in both runtimes (the reviewer confirmed the implementor's Wasmtime 49 claims in the crate source). PDD024 puts no async condition on run-time links; the Rust-partner scenarios will need async contracts too.

## Dispatch log
- 2026-09-28 00:07 PDT: implementor `card-aea6dd-383529a3` dispatched (after the weekly GC; runs beside 7bf3c5 and ff3743).
- 2026-09-28: the drive exited after the guest nudges ran out (00:3x PDT) while the agent kept working; the agent finished at 08:43Z with a full summary filed as a non-terminal report instead of `report done`. Treated as done at `d063e00bb` (run-time link through both runners, scenario 8, record lines; `tests all` green at `45823632e`, `lint` green at `d063e00bb`, which only adds a rustfmt commit). Scenario 8 passes on Wasmtime and stops at `parse` on web and native. Design notes: a run-time link only works for async-typed functions (Wasmtime 49 panics on a nested `run_concurrent`; both refuse a concurrent host function for a sync-typed import), so scenario 8 uses `greet: async func`; the polyfill can report `instantiate` where Wasmtime reports `link`; composition links in `wiring.txt` are refused until the wac step exists. Implementor paused.
- 2026-09-28: reviewer `review-aea6dd-f65e025b` launched; delivered tip `d063e00bb`.
- 2026-09-28: bounced to `card-aea6dd-383529a3` (resumed) with review round 1; reviewer paused.
