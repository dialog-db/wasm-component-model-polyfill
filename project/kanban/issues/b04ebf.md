---
id: b04ebf
title: The trap texts of a dropped end name the stream or the future, as in Wasmtime 50
type: feature
blocked_by: []
labels: [crates-io, wasmtime-50]
created: 2026-10-20T19:12:25Z
disposition: accepted
disposition_at: 2026-10-21T06:06:00Z
---

## What to build

Wasmtime 50 names the direction and the kind of end in the traps for a
dropped peer (`423da9bcb`, `trap_encoding.rs:284-296` at `v50.0.0`):

- "cannot read from stream after being notified that the writable end
  dropped"
- "cannot write to stream after being notified that the readable end dropped"
- "cannot write to future after previous write succeeded or readable end
  dropped"
- "cannot read from future after previous read succeeded"

The polyfill keeps the old texts (`error.rs:1613`, `:1619`) and gives a future
end the stream wording (`executor/copy.rs:486-491`). Wasmtime also removed the
prefix leniency from its wast runner. The polyfill's harness copies that
leniency (`tests/conformance.rs:1346-1373`), as PDD021 decided. It goes too.

## Acceptance criteria

- [x] The four texts equal Wasmtime 50's, and a future end uses the future
      wording.
- [x] The harness matches the full text, and PDD021 says so.
- [x] The Display tests and the no-provider reasons that quote the old texts
      are updated.

## Review notes

- 2026-10-20: `feat: Name the stream or the future in the traps of a done
  end`. The texts are Wasmtime 50's, and the error tests pin them to
  `Trap::ReadFromDroppedStream`, `WriteToDroppedStream`,
  `WriteToDroppedFuture`, and `LiftDroppedStream`. A done end is worded by its
  kind alone, so `CopyEnd::notified_dropped` is gone. The harness relaxation
  went with the corpus commit. PDD021's text waits for 7daabe.
