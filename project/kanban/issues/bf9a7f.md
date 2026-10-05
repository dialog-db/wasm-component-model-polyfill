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
- [ ] No reference result global outlives the call that set it; a test shows a later-stretch return releases them.
- [ ] The token doc or type is accurate.
- [ ] The bench README carries the timer guidance.
- [ ] `tests all` and `lint` pass.

## Review notes

