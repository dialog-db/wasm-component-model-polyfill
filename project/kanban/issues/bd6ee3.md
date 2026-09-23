---
id: bd6ee3
title: A host writes a stream and a future through producers
type: feature
blocked_by: [ad5dd3, a31891]
labels: [PDD021, concurrency, api]
created: 2026-09-23T06:00:00Z
---

## What to build
Build the host's writing side, named as Wasmtime names it. The traits `StreamProducer<T>` and `FutureProducer<T>` (one public type per module under `src/concurrency/`) are polled by the scheduler inside a turn with the store context of the store that owns the end, a `Destination<'_, Item>` for a stream, and a `finish` flag; the bound is `Send` natively and absent in the browser under one declaration, as `HostFuture` in `src/concurrency/host_future.rs` is bound. `Destination` is the polyfill's own view over `Vec<T>`: `remaining` is `Some(count)` when the reader is a guest and `None` when it is the host, `set_buffer` stores a vector whose items reach the reader after the poll returns, and `take_buffer` takes it back so a producer can reuse its allocation. `StreamResult { Completed, Cancelled, Dropped }` is a stream poll's result. The contract is Wasmtime's: a producer with items writes them and returns `Completed` when it can produce more or `Dropped` when it cannot; items beyond the reader's capacity stay with the end and satisfy later reads before the producer is polled again; a producer with nothing ready stores the waker and returns pending; a zero-length read reaches it as a destination with no remaining capacity, and it can return `Completed` at once or wait for readiness; `finish` is true when the guest cancelled the copy, and the end must then return ready as soon as it can, `Cancelled` when it took nothing, and can return pending once more to finish work it started; a poll that returns an error fails the guest's built-in with that error, as a host task's failure fails its subtask; any future whose output is a `Result` is a `FutureProducer`, so a host writes a future with an `async` block. `StreamReader<T>::new` and `FutureReader<T>::new` take a store context and a producer, create the shared record with the producer as its writable side, and return the readable end; `Store<T>` reaches them through its context and a host `async` function through `with` on the accessor of PDD020. A host end runs as a host task (`src/concurrency/host_task.rs`, `host_task_set.rs`): when a guest starts a copy against it, the trampoline polls the end once with the active turn's waker, and a ready poll completes the copy before the built-in returns with no blocked sentinel; a pending poll joins the store's host tasks and the guest sees the sentinel or blocks. A later turn polls with the driver's waker, moves the items through the boundary context into the guest's buffer, and fills the end's event. Both readers implement `ComponentValue` (`src/linker/component_value.rs`), so a typed host function of PDD008 and a typed call of PDD010 return them, and lowering one into a guest inserts a readable end that shares the record with the producer; lowering into a guest whose payload type differs from the projection of `T` fails with the type mismatch of PDD010.

## Acceptance criteria
- [ ] A `StreamReader<u8>` over a producer feeds a guest's asynchronous reads across several turns, proved by a repository test.
- [ ] Items beyond the guest's capacity wait for the next read, and a zero-length read reaches the producer with no capacity, each proved by a test.
- [ ] A `FutureReader<T>` over an `async` block resolves a guest's read, proved by a test.
- [ ] A producer error fails the guest's built-in with that error, proved by a test.
- [ ] In the browser the producer awaits a JavaScript promise and the guest receives the items.
- [ ] Lowering a typed reader into a guest of another payload type fails with the type mismatch, proved by a test.
- [ ] `lint` passes and `tests all` is green on both targets.

