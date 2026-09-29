---
id: fe8bc2
title: The browser backend reports link errors without reading V8 message text
type: bug
blocked_by: [f9f268]
labels: [runtime-layer, PDD025]
created: 2026-09-29T07:32:47Z
---

## What to build

The browser backend (`rust/wcmp-wasm-core-web`, landed from f9f268) finds the failing import of a `LinkError` by parsing V8's "Import #N" text (`src/errors.rs:210-224`). JavaScriptCore and SpiderMonkey messages carry no such index. So in Safari and Firefox a wrong-type import becomes `Error::Backend` (`errors.rs:244`), and a missing namespace becomes `TypeMismatch`, where both should be `Error::Link`. Wrong-kind imports are checked before the call (`store.rs:74-78`), so that case already works in every engine.

Map every `LinkError` to `Error::Link`, and name the import where the backend can know it without parsing engine text: for example, check each import against the known type of the export it receives before the call. The project weighs Safari above Firefox, so check the JavaScriptCore message shapes first.

Found by the independent review of f9f268 (finding F2).

## Acceptance criteria
- [ ] A `LinkError` in any engine gives `Error::Link`, never `Error::Backend` or `TypeMismatch`.
- [ ] Where the backend names the import, it does not depend on V8's message text.
- [ ] A web test covers a wrong-type function import and a missing namespace, and the assertions do not depend on V8's wording.
- [ ] `tests all` and `lint` pass.

## Review notes


## Dispatch log

- 2026-09-29: the trap-kind table from 10f282 (`769a2f56c`) is landed; `errors.rs` and `traps.rs` are its code. Card edf2c7 is in flight in the same crate (`calls.rs`, `owner.rs`, `flight.rs`).
- 2026-09-29: implementor `card-fe8bc2-febeb164` dispatched.
- 2026-09-29: implementor reported done at `2a553c3dd` (no engine text is read for link errors; new `linking.rs` names the failing import from the imports object and known extern types, with a conservative subtype check used only to name, never to refuse; every `LinkError` gives `Error::Link`, with empty names when no import explains it); `tests all` (web 1531/1531) and `lint` green. Design notes for the owner: PDD025 says the layer checks no subtypes, and `Error::Link` can now carry empty names. A 3-line `owner.rs` change may conflict with edf2c7. Implementor paused. Reviewer `review-fe8bc2-80e5de0a` launched; branch delivered.
