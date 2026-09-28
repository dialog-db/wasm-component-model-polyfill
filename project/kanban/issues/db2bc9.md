---
id: db2bc9
title: Keep tests zena gating under an inherited regenerate variable, and test its browser parsing
type: bug
blocked_by: []
labels: [zena]
created: 2026-09-28T05:22:00Z
---

## What to build
Close the gaps the independent review of card 568112 found in `tests zena` (landed as `179f08a0c`). None blocked the card.

- **An inherited variable skips the gate.** When `WCMP_ZENA_REGENERATE` is set in the caller's environment, the three-subject test writes the record instead of gating it (`flake.nix:945-948`, `tests/zena.rs:361`). With a wrong `web` stage and the variable set, `tests zena` passed. Unset the variable everywhere except inside `tests zena regenerate`.
- **Browser report parsing has no test.** Nothing automated covers `browser_reports` (`tests/zena.rs:313-330`) or the check that the browser run covers every scenario. The reviewer checked empty, mislabelled, duplicate, wrong-stage and garbage input by hand. Turn those into tests.
- **Menu text.** The `tests zena` menu description is very long; shorten it.

The three-subject test runs only in `tests zena` and `tests all`, not in `tests native debug` (`.config/nextest.toml:15`), so it can go stale between `tests all` runs. Clippy still compiles it. This card does not change that.

## Acceptance criteria
- [ ] With `WCMP_ZENA_REGENERATE` exported in the shell and a wrong stage in the record, `tests zena` fails.
- [ ] Each malformed browser-output case above has a test that fails when its check breaks.
- [ ] The `tests zena` menu description fits on one line of the menu.
- [ ] `tests zena` and `tests native debug` pass.
- [ ] `lint` passes.

