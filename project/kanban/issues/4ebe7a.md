---
id: 4ebe7a
title: One capability refusal search, and tests that pin the feature mapping
type: chore
blocked_by: [b5a406]
labels: [runtime-layer]
created: 2026-09-30T15:59:04Z
---

## What to build

Non-blocking findings from the independent review of b5a406 (translator features from capabilities, landed with this card's parent), in `rust/wcmp/src/executor/translator_features.rs`:

- **Two refusal searches.** The polyfill's `missing_capability` and the Wasmi backend's `wcmp-wasm-core-wasmi/src/refusal.rs` each search for the capability a refused module needs. Share one. The `.or(missing.last())` fallback (`:113`) can name a capability the component does not need when no single capability is necessary on its own (the either-of case); drop it or make it pick a needed one.
- **Mapping drift.** `feature_of` has a wildcard (`:53`), so a new lexicon feature would silently keep the config value. Derive `feature_of` from `Capability::is_wasm_feature` and `WasmFeatures::from_name`, or add a test that `feature_of(c).is_some() == c.is_wasm_feature()` for every `Capability::ALL`. `it_enables_each_capability_the_backend_declares_and_no_other` (`:298`) looks flags up through `feature_of` itself, so it cannot catch a wrong mapping; use `WasmFeatures::from_name` as an independent oracle.
- **Missing tests.** `Unsupported(exceptions)` and `Unsupported(threads)` on Wasmi; a component whose own core module has two memories (defined or imported) refused in a browser without `multi_memory`; the either-of case.
- **Off-lexicon features.** `extended_const`, `wide_arithmetic` and `compact_imports` are always on in the translator (`EngineConfig` has no setter for core features). On Wasmi, a component using `wide_arithmetic` translates, then fails at compile with `Error::Compile` rather than `Unsupported` before compile. The owner decides whether these become lexicon names (PDD025 says every feature above Wasm 2.0 is a capability; the faithfulness review of 7ae4a0 raised `extended_const` too) or are turned off in the translator.

## Acceptance criteria
- [ ] One refusal search serves the polyfill and the Wasmi backend, and it never names an unneeded capability.
- [ ] A drift test or a derived mapping holds the feature mapping to the lexicon.
- [ ] The missing tests exist.
- [ ] The owner's decision on the off-lexicon features is carried out.
- [ ] `tests all` and `lint` pass.

## Review notes

