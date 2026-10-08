---
id: 0ef2ae
title: Try Chromium with wasmfx and the web suite under WebKitGTK
type: chore
blocked_by: [fbe4df]
labels: [runtime-layer, backlog-burndown-002]
created: 2026-10-13T18:44:17Z
---

## What to build
From the 2026-10-13 review of fbe4df. The survey names two cheap routes to more fidelity that need no new backend. Try them before choosing a fourth backend.

- **Chromium with `--js-flags=--wasm-wasmfx`.** The web lane's Chrome would then turn on V8's experimental stack switching, the one capability the browser lane lacks. Check whether the browser backend's probes see it, and what the conformance and fidelity suites do with it.
- **The web suite under WebKitGTK.** The flake already has a WebKitGTK smoke driver (`tests smoke webkit`). Run the web test suite under it, as a JavaScriptCore lane with Safari's semantics. The project weighs Safari above Firefox.

This card records findings; it does not add a permanent lane unless the owner asks.

## Acceptance criteria
- [ ] For each route, a note on this card says whether it works, what it costs to run, and the conformance and fidelity results or the reason they could not be measured.
- [ ] The owner has what they need to choose between V8 (full fidelity on Chrome's engine) and JavaScriptCore (Safari parity, no stack switching).

