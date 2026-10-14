---
id: ee0ef5
title: Correct the engine survey's JSC memory64, ranking and count claims
type: docs
blocked_by: [fbe4df]
labels: [runtime-layer, backlog-burndown-002]
created: 2026-10-13T18:44:17Z
---

## What to build
From the 2026-10-13 review of fbe4df. Corrections to `project/research/Wasm Core Engine Survey.md`.

- **JSC memory64.** At WebKitGTK 2.54.1, `useWasmMemory64` is `status: testable` and "only supported in the IPInt tier". The verdict "with `JSC_useWasmMemory64=1` … the other eight features" leaves out that the flag gives interpreter-only, non-stable memory64. Add a sentence; it weakens the Safari-parity case for memory64.
- **Ranking rule.** State that candidates are ordered by fidelity first, then by cost. V8 is first on fidelity though its cost is highest, and WasmEdge sits below SpiderMonkey on fidelity, not cost.
- **Host-suspension contract.** The note says it has three rules. PDD025 lists four; the store-drop rule is missing.
- **Line count.** "12,813 lines" for the browser backend includes its tests. `src/` alone is 9,540 at the pin. Say which.
- **V8 threads cost.** One isolate per thread is not in the cost ranking.
- **Activity counts** ("48 commits since 2026-07-01", release counts) cite no source. Cite them or drop them.

## Acceptance criteria
- [x] Each point above is fixed or answered in the note.
- [x] `markdown format` and `lint` pass.

## Implementation notes

- 2026-10-13 (`docs(research): Correct the engine survey's memory64, ranking
  and counts`):
  - JSC memory64: the summary, the JSC verdict and the ranking each say the
    flag gives `testable`, IPInt-only memory64.
  - Ranking: retitled "Ranked", and it states fidelity first, then cost.
  - Contract: four rules, with the store-drop rule (PDD025 434-444).
  - Lines: 12,813 with tests at the note's commit, 9,540 in `src/`.
  - V8: one isolate per thread is a cost bullet and part of the ranking.
  - Activity counts: no source was at hand, and checking would mean
    cloning each upstream, so every uncited commit and release count is
    dropped (V8, JSC, SpiderMonkey, WasmEdge, Wasmer, WAMR, Wizard, Wasm3).
