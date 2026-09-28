---
id: 5ccfd2
title: The faithfulness suite runs on the browser backend
type: feature
blocked_by: [7ae4a0, 215523, 10f282]
labels: [PDD025, runtime-layer]
created: 2026-09-28T21:45:56Z
---

## What to build

Step 1 of the migration (build beside). Run the faithfulness suite on the browser backend in the web lane, for Wasm 2.0 and for each capability the browser declares.

Each expected failure cites a defect of the engine. If a feature has failures that no engine defect explains, the browser backend does not declare that capability.

## Acceptance criteria
- [ ] The faithfulness suite runs on the browser backend in the web lane.
- [ ] The browser backend passes the floor scripts and the scripts of each capability it declares, apart from cited expected failures.
- [ ] Each expected failure cites an engine defect.
- [ ] `tests all` includes the browser run.
- [ ] `lint` passes.

