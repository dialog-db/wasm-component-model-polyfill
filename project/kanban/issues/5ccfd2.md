---
id: 5ccfd2
title: The faithfulness suite runs on the browser backend
type: feature
blocked_by: [7ae4a0, 215523, 10f282]
labels: [PDD025, runtime-layer]
created: 2026-09-28T21:45:56Z
disposition: accepted
disposition_at: 2026-10-02T06:07:52Z
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
- 2026-09-29: implementor reported done at `75f60669f` (new `tests faithfulness web` leaf in `tests all`; browser 264/264 with 83 expected failures, each cited at V8 6cb2fb51: 75 truncation traps from the owner's known ambiguity, `array_init_elem.wast:92-93` check order, V8's table and memory64 implementation limits, and a refused main-thread atomic wait; three backend fixes: floats through the carrier as bits, V8 segment-bounds traps, two V8 link-error texts read as `Error::Link`). `tests all` and `lint` green; the seed gate passed. Owner decisions: the `array_init_elem` check order is a new V8 defect, and the implementation limits and main-thread wait are V8 by design; each is cited rather than skipped, and a dedicated-worker run could remove the wait failures. The link-error text change overlaps fe8bc2, which removes all reading of V8 link text. Implementor paused. Reviewer `review-5ccfd2-cb271992` launched; branch delivered.
- 2026-09-29, round 1 (`review-5ccfd2-cb271992` at `75f60669f`): **accept.** `tests all` (faithfulness web 264/264) and `lint` green; every V8 citation checked at 6cb2fb51; the 75 truncation entries are exactly the owner's known ambiguity; `array_init_elem.wast:92-93` is a real V8 deviation. Findings: F1, six entries are embedding limits, not V8 defects (JS API implementation-defined limits `index.bs:2208-2234`; the threads proposal's main-thread `Atomics.wait`), so Safari and Firefox fail them too; F2, the V8 link-text naming is redundant once fe8bc2 lands; F3, `carrier.rs` `function()` swallows any instantiate error; NaN payloads still fall back to a Number for non-final types, float plus concrete ref, untyped funcrefs, and `WebAssembly.Global`; every f64 now crosses as a BigInt (bench it); a Safari lane needs per-engine expected-failure lists, JSC segment-trap rows and a macOS driver; a dedicated-worker run should clear the atomic wait entries. Owner ruling on F1: keep them as expected failures, cited to the JS API and threads spec text. Before landing, the paused implementor was resumed to merge the host tip `35fa36a53` (errors.rs conflicts with fe8bc2: drop the V8 link-text parsing), re-cite the six entries, and narrow the swallowed carrier error (F3). Reviewer paused for a delta re-review.
- 2026-09-29: implementor pushed `80607fa89`: merged `35fa36a53` as `ba7328fd1` (took fe8bc2's `errors.rs` whole; the V8 link-text parsing and its test are gone); re-cited the six embedding-limit entries to WebAssembly/spec@608711107b7f `js-api/index.bs#L2208-L2234`, WebAssembly/threads@cc535ada1 `Overview.md` and tc39/ecma262@726ec8a4 (the parser now takes several citations before the reason); the carrier falls back only on a `LinkError`, with a test. `tests all` (web 1538/1538, faithfulness web 264/264) and `lint` green. Implementor paused; branch re-delivered to `review-5ccfd2-cb271992` for a delta re-review.
- 2026-09-29: merge re-review (`review-5ccfd2-cb271992` at `80607fa89`): **accept.** `tests all` (faithfulness web 264/264, web 1538/1538) and `lint` green; `errors.rs` is byte-identical to fe8bc2's, and every file only one side changed is identical to that side; all re-citations correct at their pinned commits; the uncited-entry rule still holds; the carrier falls back only on a `LinkError`. Landed as `61b255485` (clean). Follow-ups: 841af1 (worker lane, NaN fallbacks, f64 cost, per-engine lists for Safari, stale citation wording). Revert artifact: `sandbox-guest/card-5ccfd2-db322824`.
