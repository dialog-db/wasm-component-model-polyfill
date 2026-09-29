---
id: fe8bc2
title: The browser backend reports link errors without reading V8 message text
type: bug
blocked_by: [f9f268]
labels: [runtime-layer]
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

