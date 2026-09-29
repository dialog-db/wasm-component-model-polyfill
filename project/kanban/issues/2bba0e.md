---
id: 2bba0e
title: Close the Wasmtime run's review gaps
type: chore
blocked_by: []
labels: [zena]
created: 2026-09-27T21:47:29Z
---

## What to build
Close the gaps the independent review of card 17e40b found in `rust/wcmp-scenario-wasmtime` (landed as `4d3706559`). None blocked the card.

- **A result the model cannot hold.** `value.rs:42` and `wasmtime_run.rs:159` turn a result value the scenario model cannot represent into a failed call. An entry that expects `fail`, or has no outcome, would then record a failure where the call succeeded. Record this case so that it cannot pass as an expected failure.
- **Error variants.** `program.rs:28` says a program with status 0 and no `.wasm` gives `Error::Layout`, but `program.rs:41` gives `Error::Io`. `scenario.rs:48` checks for a sources directory with no compiled directory, but not the reverse, which gives `Error::Io`. Make the code and its doc comments agree.
- **The standard-output cap.** The `STDOUT_CAPACITY` comment says a write past the cap fails in the guest. A p2 write traps; a p3 write keeps a partial write and returns an error code. Correct the comment.
- **Tests.** Add a unit test of standard-output capture on a WAT component, one with p2 and p3 writes in order, and tests of the `Scenario::find` and `Program::read` layout errors.

## Acceptance criteria
- [ ] A successful call whose result the model cannot hold does not satisfy an entry that expects a failure or has no outcome.
- [ ] The layout errors and their doc comments agree.
- [ ] The standard-output cap comment states what p2 and p3 do.
- [ ] The new unit tests pass in `tests native debug`.
- [ ] `lint` passes.

