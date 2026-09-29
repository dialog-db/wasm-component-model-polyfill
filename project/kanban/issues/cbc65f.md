---
id: cbc65f
title: Name a failing browser import only when the backend can prove it
type: chore
blocked_by: [fe8bc2]
labels: [runtime-layer]
created: 2026-09-29T21:53:23Z
---

## What to build

Findings from the independent review of fe8bc2 (browser link errors, landed `508bf65b7`), in `rust/wcmp-wasm-core-web/src/linking.rs` and the core `Error::Link`:

- **Step 3 can name the wrong import.** "Exactly one import could not be checked, so name it" (`linking.rs:77-81`) is wrong when the engine refuses for another reason: JavaScriptCore's "couldn't create Table", "failed to initialize Table" or a callee-group LinkError, with one funcref import of unknown type. It also misfires because `boundary.rs:185` `FuncType` drops the rec group, supertypes and finality, so `func_fit` and tag equality say "links" for types an engine refuses (JSC compares tag RTTs by identity). Drop step 3, or resolve an unknown import by probe-instantiating a generated one-import module, so the engine decides without text.
- **`Error::Link` with empty names.** `Display` renders "the import   does not link: ..." and the variant doc says "an import was given an extern of the wrong kind or type", which is untrue for JSC's table errors. Give empty names their own `Display`, or make the names an `Option`. The owner decides whether PDD025's error list (and its "the runtime layer does not check subtypes" line) should say the browser backend compares types only to name an import.
- **Missing tests.** Web tests against the real engine for a wrong-type global (mutability, content), table (element, too small, maximum), memory (too small, shared) and tag; a LinkError about no import with one unknown import; an instantiation `TypeError` with the namespaces intact staying `TypeMismatch`; concrete-handle cases in `heap_subtype`. `with_instantiate` should restore `WebAssembly.instantiate` if the body panics.

## Acceptance criteria
- [ ] The backend never names an import it cannot prove fails.
- [ ] `Error::Link` with no import reads sensibly and its doc is true.
- [ ] The missing tests exist and run in the web lanes.
- [ ] `tests all` and `lint` pass.

## Review notes

