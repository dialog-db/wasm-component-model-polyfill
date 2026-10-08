---
id: bf9a7f
title: Release browser entrance references when a later stretch returns
type: chore
blocked_by: [f87de5]
labels: [runtime-layer, backlog-burndown-001-q4]
created: 2026-10-02T04:38:09Z
---

## What to build

Non-blocking findings from the independent review of f87de5 (browser per-call gaps):

- **Pinned references.** A resumable call that returns in a later stretch leaves its references in the entrance's reference result globals until the entrance's next first-stretch read, which may never come, pinning them for the store's life. Clear them when the flight's promise settles.
- **Token wrap.** `entrance.rs:151` says token 0 is never issued; the token is the flight id as `u32`, so it wraps to 0 at id 2^32 and tokens alias mod 2^32. Say "until the ids wrap", or widen the token.
- **Bench guidance.** In headless Chrome, `performance.now()` steps in 100 µs; document `target-sample-ms >= 50` for comparing benchmarks of a few hundred µs.
- **Older gap.** `list-u8-roundtrip/65536` is about 1-2% over `afc71435f`, from before this card.

## Acceptance criteria
- [x] No reference result global outlives the call that set it; a test shows a later-stretch return releases them.
- [x] The token doc or type is accurate.
- [x] The bench README carries the timer guidance.
- [x] `tests all` and `lint` pass.

## Review notes
- 2026-10-08 delivered (host session, backlog-burndown-001-q4), commit
  `fix(web): Release entrance references when a later stretch returns`.
  - `Entrance::release_when_settled` clears the reference result globals when
    the promise of a call that did not return in its first stretch settles,
    fulfilled or rejected, holding the entrance weakly. `start_resumable` calls
    it before `flight.watch`. The handlers run on a microtask, where no call of
    the store sits between its return and the host's read.
  - Test `entrance::tests::it_releases_the_references_of_a_call_that_returns_in_a_later_stretch`
    (web lane): a call suspends on a JSPI import and returns its externref in a
    later stretch. Unwatched, the global still holds the externref after the
    promise settles; watched, it holds `null`.
  - Token: `(id as u32).max(1)`, so no call has the token `0` that `$done`
    starts with; the doc states when two calls' tokens can alias.
  - Bench README: `target-sample-ms=50` or more for benchmarks of a few hundred
    microseconds in headless Chrome (one 100 us step is 2% of a 5 ms batch).
  - Older gap (`list-u8-roundtrip/65536` about 1-2% over `afc71435f`): not
    investigated; no criterion asks for it.
- 2026-10-08 gate on the series tip (`fix(zena): Move the Zena input back
  from the fork to Zena`): `tests all` green on every lane (native debug and
  release 1730, web debug and release 1716, no-provider 169 on each target,
  Wasmi 1447, fidelity 264/267/264, Zena Browser 18/18, Native 18/18, Wasmi
  2/18, Wasmtime 18/18, demo 68/68), with no `unexpected:` or `stale
  expectation:` line, and the full `lint` green. Conformance: native
  2412/2443, browser 2407, Wasmi 2352, no provider 2356.
- 2026-10-08 context-free review, round 1: added
  `entrance::tests::it_releases_a_later_stretch_return_of_a_store_call`, which
  drives a suspending call through the public `Store` and counts the settle
  release (a test-only counter); with the `start_resumable` call removed it
  fails.
- 2026-10-08 context-free review, round 4: **accepted**, with no code finding.
  The `token` doc now names every aliasing case, the adjacent flights `k *
  2^32` and `k * 2^32 + 1` included.
