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


## Dispatch log

- 2026-09-29: the seed is the first tip that carries 10f282 (`769a2f56c`, trap kinds) on top of 4694aa (`16c966ff9`, Wasmi suspension); their contract test lists auto-merged. If the gate fails on the seed before your change, say so in your report and name the failure. The web trap fixture carries the owner's `AMBIGUOUS` allowance for V8's truncation message; treat that as the precedent for any browser-engine ambiguity the spec tests expose, and report any new one rather than widening it.
- 2026-09-29: implementor `card-5ccfd2-db322824` dispatched.
