# Streams and Futures

[PDD018] designed the runtime that every concurrency feature shares. [PDD019]
built the callback export on it, and [PDD020] built the subtask and the
asynchronous import. This document designs the third feature: `stream<T>` and
`future<T>`. A stream carries a sequence of values from one party to another,
and a future carries one value. Each has a readable end and a writable end. The
feature brings the fourteen built-ins that create, read, write, cancel, and drop
the ends. It brings the two value types in a function type and the transfer of a
readable end between components. It also brings the Rust types through which a
host reads and writes a guest's stream or future.

The design follows the [Concurrency explainer][Concurrency] and the Python
reference in [`definitions.py`] at the commit the conformance corpus of [PDD016]
is vendored from. Where the reference leaves a choice to the host, the design
makes the choice [Wasmtime] makes at `v49.0.0-rc.1`, and it spells its public
names as Wasmtime spells them. Where the design departs from the reference or
from Wasmtime, it states the reason.

Two terms recur. A copy is one read or one write on one end. The reference uses
the word for both kinds, in `stream_copy`, `future_copy`, and `cancel_copy`. A
host end is a stream or future end the host holds, as opposed to a guest end,
which is an entry in a component instance's handle table.

## Goals

- The type projection accepts `stream<T>` and `future<T>` in a function type, on
  an import and on an export, and the public `ValueType` names them with their
  payload type.
- A guest creates a stream or a future and gets both ends in its handle table.
  The entries take the kinds the runtime model reserved for them.
- A read and a write on the two ends of one stream pair up as the reference's
  stream state pairs them. Partial copies, zero-length copies, and the packed
  result follow the reference. A future carries one value and each end is used
  at most once.
- A copy on a payload of a number type, or with no payload, copies bytes between
  the two memories with no value built per element. The same set of payloads is
  the one set that allows a read and a write from one instance.
- A copy with the `async` option returns the blocked sentinel and completes
  through an event on the end, which the waitable sets of [PDD019] deliver. A
  copy without the option blocks under the rules of [PDD020].
- A copy cancel ends one pending copy on one end and reports the progress the
  copy made.
- A readable end crosses a boundary as a value. It is lifted out of the sender's
  table and lowered into the receiver's table. The transfer intrinsics of the
  fused adapters move it between two components. The writable end never leaves
  the instance that created it.
- A host creates a stream or future from a producer, reads one with a consumer,
  or closes it, through types named as Wasmtime names them. An untyped value
  carries a stream or future as `Val::Stream` and `Val::Future`.
- Every crossing has a copy budget with Wasmtime's default and message, so a
  guest cannot make the host build a value without bound.
- The translator accepts `task.cancel` and `subtask.cancel` and fails each one
  only when a guest calls it. A guest whose binding layer links them therefore
  runs its paths that never cancel.
- Every trap the feature adds is a structured error cause with Wasmtime's
  message, so the conformance corpora match it by substring.
- The behavior is the same on both targets.

## Non-goals

- Cancellation of a task or a subtask. `task.cancel` and `subtask.cancel` are
  accepted at translation and fail at the call with `Error::Unsupported`. The
  cancelled event is never delivered. A copy cancel is not task cancellation.
- Error contexts, as built-ins and as a value type. They stay refused at
  translation.
- The stackful form of `canon lift async` and the thread built-ins other than
  `thread.yield`. They stay refused at translation.
- A synchronous copy that only the caller below it on the stack can release.
  That shape needs a stack switch, and the corpus files that need it stay
  deferred with that reason.
- An untyped read or write on `StreamAny` or `FutureAny`. Wasmtime holds those
  two types to close and typed conversion and has stated that it will relax the
  restriction. The polyfill follows that trajectory when Wasmtime fixes the
  shape, so that the names match.
- A view into guest memory for a host producer or consumer. The runtime layer
  offers only a copy in and a copy out of a memory. The host buffer types of
  this design are the surface where such an optimization lands when the layer
  allows one. Nothing here prevents it.
- The rules that decide which trap poisons an instance.
- A provider for the suspend seam.

## The Two Value Types

The type projection of [PDD006] accepts `stream<T>` and `future<T>` on an import
and on an export. `ValueType` gains two variants, `Stream` and `Future`, each
with an optional payload type. Wasmtime exposes the same two as
`types::StreamType` and `types::FutureType`. Validation has already refused a
payload that contains a `borrow`, and it refuses `stream<char>`, which the
Explainer marks as a temporary rule. The polyfill adds no check of its own. The
flat representation of either type is one `i32`, the index of a readable end in
the handle table of the instance that holds it.

Both types are waitables, so a function type that carries one is ordinary in
every other respect. A synchronous export can take or return a stream. A
synchronous host function can too. The value that crosses is always a readable
end.

## The Ends and the Shared Record

Each stream or future is one shared record in the store, and each of its two
ends is one end record. A handle table entry of the kind `StreamReadable`,
`StreamWritable`, `FutureReadable`, or `FutureWritable` holds the index of an
end record, as a subtask entry holds the index of a subtask record. The kinds
were reserved by [PDD018]. This document defines them.

An end record holds:

- The waitable state of [PDD018]: one pending event slot, the set the end
  joined, and the synchronous-waiter flag.
- The copy state: `idle`, `copying`, `cancelling`, or `done`. These are the
  reference's `CopyState`.
- The direction, readable or writable, and the index of the shared record.
- The buffer of the copy in progress, when the end is copying. For a guest end
  the buffer is a boundary context, a pointer, a length, and the progress made.
  For a host end it is the producer or consumer and the items it delivered but
  the other side has not taken.

The shared record holds the payload type, whether either end was dropped, and
the pending side: the end whose copy is waiting for the other end, if any. The
two ends of a guest-created stream start in the creating instance's table. The
readable end moves when it crosses a boundary. The writable end stays, which is
the reference's rule that a writable end is permanently owned by the calling
instance.

```text
fn stream_new(builtin):
    inst = current_instance()
    trap_if(not inst.may_leave, CannotLeave)
    shared = store.copies.insert(Shared(payload = builtin.payload))
    readable = store.ends.insert(End(readable, shared))
    writable = store.ends.insert(End(writable, shared))
    ri = inst.handles.insert(StreamReadable(readable))
    wi = inst.handles.insert(StreamWritable(writable))
    return ri | (wi << 32)
```

`future.new` does the same with the future kinds. The two indices come from the
instance's allocator in that order, readable first, so a guest that reads them
apart from the `i64` sees consecutive indices on a fresh instance. Both
built-ins trap when the may-leave flag is clear, which is Wasmtime's
cannot-leave cause of [PDD019].

## Reads and Writes

`stream.read` and `stream.write` take an end index, a pointer, and a count.
`future.read` and `future.write` take an end index and a pointer, and their
count is one. Each returns one `i32`. The built-in is lifted with a stream or
future type and canon options, and the options carry the `async` flag and the
memory the buffer lives in.

```text
fn stream_copy(builtin, index, ptr, count):
    thread = current_thread()
    inst = thread.task.instance
    trap_if(not inst.may_leave, CannotLeave)
    end = inst.handles.get(index)                 // traps unless the kind matches the built-in
    trap_if(end.shared.payload != builtin.payload, PayloadMismatch)
    trap_if(end.state != idle, ConcurrentOperation)
    trap_if(end.set is set and not builtin.async, SyncInWaitableSet)
    trap_if(count > 2^28 - 1, CountTooLarge)
    cx = BoundaryContext(builtin.options, inst, scope = none)
    buffer = GuestBuffer(cx, builtin.payload, ptr, count)   // checks alignment and bounds
    end.state = copying
    end.buffer = buffer
    pair(end.shared, end)                          // copies now if the other end is pending
    if end.pending_event is empty:
        if builtin.async:
            return BLOCKED                         // 0xffffffff
        block(store, condition = end.pending_event is set)
    event = end.take_event()
    return event.payload                           // result | (progress << 4)
```

The reference builds the guest buffer eagerly. When the payload is present and
the count is above zero, it checks the buffer at that point. It traps if the
pointer is not aligned for the payload or if the range leaves the memory. The
polyfill does the same through the boundary context, and the message names the
bounds as Wasmtime's copy does.

`pair` is the reference's `SharedStreamImpl.read` and `write`. Its rules:

- The first copy on a stream with no pending side makes this end the pending
  side and returns. The copy completes later, when the other end starts a copy.
- A copy that finds the other end pending copies at once. The count copied is
  the smaller of the two remaining lengths. The copy that started later
  completes with that count. The pending copy stays pending, and its buffer
  keeps accepting further copies, until its event is delivered. Its event then
  reports the total progress. This is the reference's reclaim rule, and it lets
  several small writes fill one large read before the reader runs again.
- A zero-length copy completes as soon as the other end is pending, with no
  bytes moved. The Concurrency explainer defines it as a readiness probe, and
  states that a later non-zero copy can still block.
- A copy that finds the pending side already full completes the pending copy and
  takes its place as the pending side.
- A copy on a stream whose other end was dropped completes at once with the
  dropped result and moves this end to `done`.

The completion of a copy fills the end's pending event. The event is the triple
of [PDD018]: the stream read or write code, the end's index, and the packed
result. The packed result is the reference's `pack_copy_result`: the low four
bits hold completed (0), dropped (1), or cancelled (2), and the bits above hold
the count copied. For a future the count is always zero. The blocked sentinel is
the value `0xffffffff`, the reference's `BLOCKED`, which no packed result can
equal. A synchronous copy takes the event before it returns. An asynchronous
copy leaves it for the waitable set the end joined, or for a later synchronous
copy cancel. An end in the `done` state accepts only a drop. A read after the
writable end dropped, or a write after the readable end dropped, traps with
Wasmtime's message for that direction.

### The Copy Itself

Every copy runs through the boundary context of [PDD018]. The writer's context
lifts elements from the writer's memory and the reader's context lowers them
into the reader's memory. A payload that carries an owned resource handle moves
the handle from the writer's table to the reader's table, as a call moves one.
The context has no borrow scope, because validation refused every borrow in a
payload.

When the payload is a number type, or absent, the copy moves bytes. The number
types are `s8`, `u8`, `s16`, `u16`, `s32`, `u32`, `s64`, `u64`, `f32`, and
`f64`. Every bit pattern of those types is a valid value, so a byte copy and a
value copy give the same bytes. `bool` and `char` are excluded for the reason
Wasmtime states: not every bit pattern is valid for them. Wasmtime selects the
same set in its compiler and copies the bytes in one step. The polyfill's
translator makes the selection from the payload type of the built-in, and the
copy calls the runtime layer's read of one memory and write of the other once.
Every other payload copies one element at a time through the two contexts.

The same set gates a read and a write from one instance. The reference traps, as
a temporary rule, when the two ends of one stream or future are used from the
same instance and the payload is not a number type. Wasmtime's message is the
one the corpus expects. The polyfill keeps the rule and the message.

### Futures

`future.read` and `future.write` are the same copy with a count of one and no
partial step. The differences the reference states:

- A completed copy, and a dropped result, both move the end to `done`. A future
  is written at most once and read at most once.
- A writer whose readable end was dropped completes with the dropped result. A
  reader never sees the dropped result, because a writable future end cannot be
  dropped before it has written. That is the rule under Dropping an End below.
- A write to a future in the `done` state traps with Wasmtime's message, which
  names both causes: a previous write succeeded, or the readable end dropped. A
  read in that state traps with the message that names a previous read.

## Cancelling a Copy

The four cancel built-ins take an end index and return the packed result. Each
is lifted with a type and an optional `async` flag. The reference's
`cancel_copy` holds:

```text
fn cancel_copy(builtin, index):
    inst = current_instance()
    trap_if(not inst.may_leave, CannotLeave)
    end = inst.handles.get(index)                 // traps unless the kind matches
    trap_if(end.shared.payload != builtin.payload, PayloadMismatch)
    trap_if(end.state != copying or end.synchronous_waiter, NoCopyPending)
    trap_if(end.set is set and not builtin.async, SyncInWaitableSet)
    end.state = cancelling
    if end.pending_event is empty:
        end.shared.cancel()                       // notifies the pending side with cancelled
        if end.pending_event is empty:
            if builtin.async:
                return BLOCKED
            block(store, condition = end.pending_event is set)
    return end.take_event().payload
```

A cancel on an end whose copy already completed returns that completion. Its
event was waiting in the slot, and the guest reads the completed result with the
progress. A cancel on an end that is still the pending side takes the buffer
back and returns the cancelled result with the progress made so far. The end
returns to `idle` unless the result is dropped. A cancel against a host end asks
the host to finish, as The Host Surface states. It can leave the guest blocked,
or waiting on an event, until the host answers.

## Dropping an End

The four drop built-ins remove the entry and drop the end.

- Dropping an end during a copy traps. Wasmtime's message names the kind and the
  side. A readable end fails with "cannot remove busy stream" and a writable end
  with "cannot drop busy stream". The future messages say "future".
- Dropping a writable future end that has not written traps with Wasmtime's
  message, so that a reader always gets its value. A writable future end whose
  reader dropped is in the `done` state and drops cleanly.
- Dropping the first end of a pair marks the shared record dropped and notifies
  the other end if it is pending: its copy completes with the dropped result. A
  later copy on the other end completes with the dropped result at once and
  moves that end to `done`.
- Dropping the second end frees the shared record.
- Every drop traps when the may-leave flag is clear.

A handle table entry for an end follows the table rules of [PDD018]. A drop
frees the index to the instance's free list.

## Crossing the Boundary

A stream or future value in a parameter or a result is a readable end. Lifting
one removes the entry from the sender's table. Lowering one inserts an entry
into the receiver's table. The two operations are the reference's
`lift_async_value` and `lower_stream` and `lower_future`.

```text
fn lift_readable_end(cx, index, ty):
    entry = cx.instance.handles.remove(index)     // traps unless the kind matches
    end = store.ends.get(entry.end)
    trap_if(end.shared.payload != ty.payload, PayloadMismatch)
    trap_if(end.state == done, LiftAfterDone)     // Wasmtime names the reason per kind
    trap_if(end.set is set, LiftInWaitableSet)
    trap_if(end.state != idle, LiftDuringCopy)
    return end

fn lower_readable_end(cx, end, ty):
    return cx.instance.handles.insert(Readable(kind of ty, end))
```

Wasmtime's three lift messages name the kind and the cause. The causes are that
the end was notified that the writable end dropped, that the end is in a
waitable set, and that a previous read succeeded. The reference traps on the
same three conditions with one message. The polyfill uses Wasmtime's, because
the corpus matches them.

Between two components the fused adapters of Wasmtime 49 call the
`StreamTransfer` and `FutureTransfer` intrinsics, one per value, with the source
index and the two types. The polyfill serves each as a trampoline that lifts
from the caller's table and lowers into the callee's table, as the resource
transfer intrinsics of [PDD015] move an owned handle. The index changes. The end
record does not.

Between a guest and the host the same lift and lower run inside the boundary
context of the call. A stream in a result the host reads through `Func::call`
arrives as `Val::Stream(StreamAny)` or as a typed `StreamReader<T>`, and the
guest's entry is gone. A stream in a parameter the host passes enters the
callee's table as a readable end. The result of `task.return` crosses the same
way. A host end lowered into a guest stays a host end on the writing side: the
guest's readable end and the host producer share one record.

## The Copy Budget

Every boundary context carries a copy budget. Wasmtime charges the bytes of
every value a lift builds against a budget of 128 MiB per call into the host,
and it traps when the budget is spent. A guest therefore cannot make the host
build a value without bound. The polyfill adopts the budget with Wasmtime's
default and its message, "too much data is being copied between the host and the
guest: fuel allocated for hostcalls has been exhausted". The design uses the
words copy budget for the concept. The message keeps Wasmtime's wording only so
the corpus matches it by substring.

The budget applies to every crossing, not only to a copy. Each list, string, and
map lift charges the count of its elements times the size in bytes of the host
value type it builds. Wasmtime charges the size of its `Val` the same way. A
crossing is one context, so one call, one `task.return`, or one copy has one
budget. The Wasmtime corpus proves the budget with a stream write and a future
write of a nested list whose layers alias one another, which no host can
materialize. Without the budget the polyfill attempts it.

## Blocking

A copy or a cancel without the `async` option blocks the thread when no event is
ready. That is the suspend seam of [PDD018], served by the nested turn under the
four rules [PDD020] states. Nothing in those rules changes:

- An idle store fails the built-in with the deadlock cause. A synchronous read
  on a stream whose writer never writes ends this way. The Wasmtime corpus
  expects the deadlock message from a callback export that reads a future no one
  writes.
- A pending host end counts as a pending host task. A synchronous read against a
  host producer that stays pending fails with the stack-switch cause.
- A sync-typed call in progress turns both failures into the cannot-block cause.
  The Wasmtime corpus expects that message from a synchronous export that reads
  a future synchronously.
- Ready work in other tasks runs inside the nested turn and can complete the
  copy. A callback task of another instance that writes the stream and then
  parks, which means it returns to its event loop with no frame on the stack,
  releases the reader.

The one shape the nested turn cannot serve is a copy that only the caller below
it can release. Consider a callback export that returns its result and then
writes synchronously to a stream that its own caller must read. Its frame stays
on the stack under the trampoline, and the caller cannot run. Both corpora carry
that file. It needs a stack switch and stays deferred.

## The Host Surface

The host holds a stream or future through two typed types and two untyped types,
all named as Wasmtime names them: `StreamReader<T>`, `FutureReader<T>`,
`StreamAny`, and `FutureAny`. There is no writer type. A host writes by
supplying a producer when it creates the value, and reads by supplying a
consumer when it receives one.

### Producers and Consumers

Four traits describe a host end. Each is polled by the scheduler inside a turn,
with the store context of the store that owns the end, the buffer view of the
copy, and a `finish` flag. The bound is `Send` natively and absent in the
browser, under one declaration, as `HostFuture` of [PDD018] is bound.

```rust
pub trait StreamProducer<T: 'static>: Send + 'static {
    type Item;
    fn poll_produce(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        store: &mut StoreContext<'_, T>,
        destination: Destination<'_, Self::Item>,
        finish: bool,
    ) -> Poll<Result<StreamResult>>;
}

pub trait StreamConsumer<T: 'static>: Send + 'static {
    type Item;
    fn poll_consume(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        store: &mut StoreContext<'_, T>,
        source: Source<'_, Self::Item>,
        finish: bool,
    ) -> Poll<Result<StreamResult>>;
}

pub trait FutureProducer<T: 'static>: Send + 'static {
    type Item;
    fn poll_produce(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        store: &mut StoreContext<'_, T>,
        finish: bool,
    ) -> Poll<Result<Option<Self::Item>>>;
}

pub trait FutureConsumer<T: 'static>: Send + 'static {
    type Item;
    fn poll_consume(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        store: &mut StoreContext<'_, T>,
        source: Source<'_, Self::Item>,
        finish: bool,
    ) -> Poll<Result<()>>;
}

pub enum StreamResult { Completed, Cancelled, Dropped }
```

`Destination<'_, T>` is the buffer of a read the host serves. `remaining`
returns the count the reader can still take, which is `Some` when the reader is
a guest and `None` when it is the host. `set_buffer` stores a `Vec<T>` whose
items reach the reader after the poll returns, and `take_buffer` takes the
stored vector back so a producer can reuse its allocation. `Source<'_, T>` is
the buffer of a write the host serves. `remaining` returns the count the writer
offers, and `read` moves up to a count the consumer names into a `Vec<T>`. Both
views are the polyfill's own over `Vec<T>`. Wasmtime's views carry buffer traits
and a direct byte view into guest memory. The runtime layer offers no view into
a memory, so the polyfill's views are where that optimization lands when the
layer allows one.

The contract of a poll is Wasmtime's:

- A producer that has items writes them and returns `Completed` when it can
  produce more, or `Dropped` when it cannot. Items beyond the reader's capacity
  stay with the end and satisfy later reads before the producer is polled again.
  A producer with nothing ready stores the waker and returns pending.
- A consumer that can take items takes them and returns `Completed` or
  `Dropped`. A consumer can take items and still return pending, which delays
  the writer's completion until a later poll returns ready. That is the
  backpressure of the Wasmtime contract.
- A zero-length read reaches a producer as a destination with no remaining
  capacity. The producer can return `Completed` at once or wait for readiness.
  Both are allowed, per the Stream Readiness section of the explainer.
- `finish` is true when the guest cancelled the copy. The end must then return
  ready as soon as it can, with `Cancelled` when it took nothing, and it can
  return pending once more to finish work it started.
- A poll that returns an error fails the guest's built-in with that error, as a
  host task's failure fails its subtask.
- Any future whose output is a `Result` is a `FutureProducer`, so a host writes
  a future with an `async` block, as in Wasmtime.

A host end runs as a host task of [PDD018]. When a guest starts a copy against a
host end, the trampoline polls the end once with the waker of the active turn.
If the poll is ready, the copy completes before the built-in returns and the
guest sees the result without the blocked sentinel. If it is pending, the end
joins the store's host tasks, and the guest sees the blocked sentinel or blocks
under the rules above. A later turn polls the end again with the driver's waker,
moves the items through the boundary context, and fills the end's event. A
stream the host created and the host piped copies from producer to consumer
inside a turn with no guest involved.

### The Readers

```rust
impl<T: ComponentValue> StreamReader<T> {
    pub fn new<D: 'static>(store: &mut StoreContext<'_, D>,
                           producer: impl StreamProducer<D, Item = T>) -> Result<Self>;
    pub fn pipe<D: 'static>(self, store: &mut StoreContext<'_, D>,
                            consumer: impl StreamConsumer<D, Item = T>) -> Result<()>;
    pub fn close<D: 'static>(&mut self, store: &mut StoreContext<'_, D>) -> Result<()>;
    pub fn close_with<D: 'static>(&mut self, accessor: &Accessor<D>) -> Result<()>;
    pub fn guard<D: 'static>(self, accessor: Accessor<D>) -> GuardedStreamReader<T, D>;
    pub fn try_into_stream_any<D: 'static>(self, store: &mut StoreContext<'_, D>) -> Result<StreamAny>;
    pub fn try_from_stream_any(stream: StreamAny) -> Result<Self>;
}
```

`FutureReader<T>` has the same entries over `FutureProducer`, `FutureConsumer`,
`GuardedFutureReader`, and `FutureAny`. `Store<T>` reaches each entry through
its store context, and a host `async` function reaches it through `with` on the
accessor of [PDD020].

`new` creates the shared record with the producer as its writable side and
returns the readable end. `pipe` sets the consumer as the reading side of a
readable end the host holds and gives the end up. `close` drops the readable
end, which completes a pending write with the dropped result and lets a later
write see it at once. Both typed readers implement `ComponentValue`, so a typed
host function of [PDD008] and a typed call of [PDD010] take and return them.
Lowering a typed reader into a guest whose payload type differs from the
projection of `T` fails with the type mismatch of [PDD010]. Lifting one from a
guest checks the same.

The lifecycle rule follows Wasmtime, with one departure. A reader the host holds
must end through `pipe`, `close`, or a lower into a guest. A reader dropped
without any of them leaks its slot until the store drops, and a guest that
writes to it waits forever. The two guard types close on drop through the
accessor they hold. A guard dropped outside a poll of its store cannot reach the
store, so its end leaks the way an unclosed reader does. This departs from
Wasmtime. Wasmtime's guard also closes through `Accessor::with`, and that call
panics when no poll of the store is running or when another call is already
inside the store. So a Wasmtime guard dropped there panics, and a failed close
trips a debug assertion. The polyfill's accessor returns both cases as errors.
Its guard lets the end leak rather than panic in a drop, because a panic in a
drop aborts the process when it runs while a thread unwinds. Dropping the store
drops every shared record, every end, and every producer and consumer without
polling them, which extends the store-drop rule of [PDD018].

### The Untyped Values

`Val` gains two variants, `Stream(StreamAny)` and `Future(FutureAny)`, so the
untyped entries of the linker and `Func::call` carry a stream or future as they
carry a resource. Each `Any` type holds the readable end and its payload type.
It offers `close` and the conversion to and from the typed reader, which checks
that `T` projects to the payload type. Lowering one into a guest checks the
payload type against the guest's. That is Wasmtime's surface at this version.
Wasmtime has stated that it will let a host read and write these values without
naming the type, and the polyfill follows that shape when it lands.

### Values That Name One End

Several host values can name one readable end. A value names an end when it
holds the identity of the end. Cloning a `StreamAny` or a `FutureAny` copies the
identity. Converting a value into a typed reader, or a reader into a value,
copies it too. Two readers decoded from one `Val` name one end as well. The
specification is silent on host values. Its `lower_stream` and `lower_future`
(`definitions.py` at commit `e5ee0af`, lines 1770 to 1778) assert only the
value's type and add a new readable end, and what the host holds is the host's
to define. Wasmtime's values are copies of one identifier, so the polyfill
follows Wasmtime wherever Wasmtime reaches a state its own code treats as sound.

The host holds an end until one value that names it lowers it into a guest,
pipes it, or closes it. Every value that names the end then sees that use. An
end that a guest hands back to the host is the host's again, and so is every
value that names it.

A use of an end that is gone fails with the not-present cause and Wasmtime's
message, "resource not present". An end is gone when both of its ends dropped.
It is gone when the host closed a stream or future it created. It is gone when a
pipe found the writer dropped or ran to its end. A value that closed names no
end afterwards, whether its close succeeded or failed.

While the end is in the store, a value that names it can do four things:

- Convert. A conversion between a typed reader and an untyped value checks only
  that the end is in the store and that the payload type matches. It succeeds
  whoever holds the end.
- Close an end that dropped already. Another value closed the end, or the guest
  the host lowered it into dropped it. A guest still holds the writable end. The
  close succeeds and changes nothing. The writer learns of the drop once.
- Pipe an end that another value piped. A guest must hold the writable end, and
  no write of it can be in flight. A write is in flight from its start until an
  end answers it. The new consumer replaces the old one. The polyfill drops the
  old consumer without a poll, and the next write polls the new consumer.
  Wasmtime's `set_consumer` does the same on a writer that is open.
- Close an end that another value piped, under the same two conditions. The end
  drops as a guest's drop does. The polyfill drops the consumer without a poll,
  and the writer receives the dropped result. Wasmtime's `host_drop_reader` does
  the same on a writer that is open.

Every other use fails. A pipe or a close fails with the not-held cause, whose
message is the polyfill's own. A lower fails as an invalid handle. The failing
uses fall into three groups.

The first group leads Wasmtime into a state that its own code rejects as a bug.
Wasmtime raises that bug as a panic in a debug build and as a trap in a release
build. The check a guest read reaches is at `futures_and_streams.rs` line 3785
at the corpus commit `cb091c33c`. The polyfill does not reproduce a bug as a
behavior, so it refuses the use first:

- A lower after a pipe, a close, or another lower. A later guest read of the new
  entry fails Wasmtime's check that the reading side is open.
- A pipe or a close after a lower. The guest that holds the end then fails its
  read the same way.
- A pipe or a close after a pipe while a write the consumer serves is in flight.
  A second pipe sets two consumers to settle one write, and the second to finish
  fails. A close drops the reading side under the running consumer, and its next
  poll fails.

The second group follows a pipe of a stream or future the host created. That
pipe joins the producer and the consumer in one host task. No later call reaches
into that task, so a second pipe or a close fails. In Wasmtime, a second pipe
there fails as a bug, and a close deletes the stream under the task that copies.

The third group is a pipe after a close. Wasmtime opens the reading side again,
for a writer that learned that the reader dropped. The drop of an end is final
in the polyfill, as it is in the specification. An end whose consumer ran to its
end drops the same way.

## Translation

The translator accepts what this design builds and refuses the rest with
`Error::Unsupported`, the rule [PDD006] states:

- Accepted: `stream.new`, `stream.read`, `stream.write`, `stream.cancel-read`,
  `stream.cancel-write`, `stream.drop-readable`, `stream.drop-writable`, the
  seven `future.*` built-ins, the `StreamTransfer` and `FutureTransfer`
  trampolines, and a stream or future type in a function type.
- Accepted with a call-time failure: `task.cancel` and `subtask.cancel`. Each
  trampoline fails with `Error::Unsupported` when a guest calls it, after the
  may-leave check. A guest whose binding layer links either built-in
  instantiates and runs every path that does not cancel. This is the one
  exception to the translation rule, and it exists so a guest built by a real
  toolchain runs before cancellation is designed.
- Refused: the error-context built-ins and the error-context type, the
  `ErrorContextTransfer` trampoline, `canon lift async` without a callback,
  every thread built-in other than `thread.yield`, and the table initializer of
  `thread.new-indirect`.

## Error Model Growth

`wcmp::Error` gains one variant for the copy built-ins, `Copy`, with a
structured cause `CopyCause`. Each message is Wasmtime's, so the corpus matches
it by substring:

- Concurrent operation: a copy on an end that is not idle. Wasmtime's
  `ConcurrentFutureStreamOp`.
- Count too large: a count of 2^28 or more. Wasmtime's `StreamOpTooBig`.
- Buffer out of bounds, and buffer not aligned: the guest buffer leaves the
  memory or its pointer is not aligned for the payload. Wasmtime's messages name
  the pointer of the direction.
- Read after done, and write after done: a copy on an end that was notified the
  other end dropped. Wasmtime's message differs per direction and kind, and the
  future messages also name a previous read or write.
- Future write end not written: a writable future end dropped before its write.
- Busy end: a drop or a lift of an end during a copy, with Wasmtime's message
  per side and kind.
- Same-instance payload: a read and a write from one instance with a payload
  outside the number types.
- Lift after done, lift in waitable set, and lift during copy: the three lift
  traps, with Wasmtime's message per kind.
- No copy pending: a cancel on an end that is not copying.
- Not held by host: a pipe or a close of a readable end that the host no longer
  holds, because a value naming the same end lowered, piped, or closed it. The
  message is the polyfill's own. The Values That Name One End section states
  which uses fail and why.
- Host end not present: a use of a value whose end left the store, or of a value
  that closed. Wasmtime's table message, "resource not present".
- Payload mismatch: the built-in's type differs from the end's. The message is
  the polyfill's own, because Wasmtime checks it through its table types.

`AbiCause` gains the copy budget spent, with Wasmtime's message, because the
budget applies to every crossing. A synchronous copy or cancel on an end inside
a waitable set fails with the waitable cause [PDD018] already carries for a
synchronous use of a waitable in a set. The cannot-leave cause of [PDD019]
applies to every built-in here. The scheduler causes of [PDD018] apply to a
blocked copy under the rules of [PDD020]. `task.cancel` and `subtask.cancel`
fail with `Error::Unsupported`.

## Target Differences

Nothing in this document differs between the native target and the browser
beyond the four differences [PDD018] states. The one this feature reaches is the
`Send` bound on a host task, which here binds a producer or a consumer: required
natively, absent in the browser, under one declaration.

## The Corpus

This feature removes the lines its scope owns from the expected-failure list and
leaves the rest with their reasons. The harness registers no new host item. The
files and directives this design owns, in the Component Model corpus:

- `builtin-trap-poisons-instance.wast`, its second component, that component's
  instantiation, and the directive that drops a busy stream. Its two directives
  that expect the poisoning trap stay deferred.
- `cancel-stream.wast`, `closed-stream.wast`, `cross-task-future.wast`,
  `drop-stream.wast`, `empty-wait.wast`, `futures-must-write.wast`,
  `partial-stream-copies.wast`, `same-component-stream-future.wast`,
  `trap-if-done.wast`, `trap-if-transfer-in-waitable-set.wast`,
  `wait-during-callback.wast`, and `zero-length.wast`, whole.
- `drop-cross-task-borrow.wast`, whole. A borrow lent to a callback task that
  waits on a future is dropped from another task of the same instance. The
  lender's return then traps or succeeds as [PDD014] states.
- `passing-resources.wast`, whole. Owned handles cross a stream, and the last
  directive expects the handle-table message the synchronous baseline gives an
  unknown index.
- `validate-no-stream-char.wast`. It passes already and proves that validation
  refuses `stream<char>`.

In the Wasmtime corpus:

- `async-builtins.wast`, `futures.wast`, and `streams.wast`, whole. Each
  component defines one built-in, and the last directives of `streams.wast`
  expect the concurrent-operation trap.
- `cancel-sync-and-waitable.wast`, its first four components and their
  directives, which expect the waitable cause from a synchronous cancel on an
  end inside a set. Its fifth component calls `subtask.cancel`, now
  instantiates, and fails at that call, so its directive stays deferred for
  cancellation.
- `future-cancel-read-dropped.wast`, `future-cancel-write-completed.wast`,
  `future-cancel-write-dropped.wast`,
  `future-drop-writable-after-notified-drop.wast`, `futures-must-write.wast`,
  `futures-must-write2.wast`, `intra-futures.wast`, `intra-streams.wast`,
  `partial-stream-copies.wast`, `stream-cancel-finished-op.wast`,
  `stream-zero-ops.wast`, `sync-and-async-waitable.wast`, `trap-if-done.wast`,
  `trap-if-transfer-in-waitable-set.wast`, and `waitable-set-stale-entry.wast`,
  whole.
- `future-read.wast`, whole. Its four directives permute a synchronous and an
  asynchronous read with a synchronous and a callback lift. The synchronous read
  from the synchronous lift fails with the cannot-block message, and from the
  callback lift with the deadlock message.
- `stream-big-read-and-writes.wast`, whole. A count of 2^28 traps with the count
  message, and a buffer past the end of memory traps with the bounds message.
- `streams-massive-send.wast`, whole, through the copy budget.
- `task-builtins.wast`, its stream and future case, and its `subtask.cancel`
  component, which now instantiates.

The lines that change without joining this design's scope, because the
translator now accepts `task.cancel` and `subtask.cancel`:

- Every file deferred for cancellation keeps its reason. A directive that only
  defines or instantiates a component which links either built-in starts to pass
  and loses its line. A directive that reaches the call fails with the
  unsupported error, and its line's reason changes to that failure. The files
  are `big-interleaving-test.wast`, `cancel-and-exclusive-lock.wast`,
  `cancel-delivery.wast`, `cancel-subtask.wast`, `cancel-host.wast`,
  `cancel-sibling-subtask.wast`, `cancel-starting-subtask-does-not-leak.wast`,
  `yield-when-cancelled.wast`, and the two cancel directives of the Component
  Model `reentrance.wast`.
- `cm/values/post-return.wast` was rejected for `task.cancel`. It is now
  rejected for `thread.index`, and its reason changes to the thread built-ins.

The files that stay deferred, with the reason:

- A stack switch: `sync-streams.wast` in both corpora, and
  `async-calls-sync.wast` as before.
- The stackful lift: `sync-barges-in.wast`, `stackful.wast`,
  `drop-waitable-set-stackful.wast`, `reenter-during-yield.wast`, five
  directives of `task-return-traps.wast`, and `big-interleaving-test.wast`,
  which is also cancellation.
- Thread built-ins: `switch-to-ready-callback.wast`,
  `trap-if-block-and-sync.wast`, `trap-if-sync-and-waitable-set.wast`,
  `join-during-sync-read.wast`, `cm/values/post-return.wast`, and every file
  [PDD020] lists for that reason.
- Cancellation: the files named above.
- Error contexts: `error-context.wast` and
  `error-context-trap-in-post-return.wast`.
- The trap rules: `builtin-trap-poisons-instance.wast`, its two poisoning
  directives.

### The WASI 0.3 HTTP Handler

A guest that implements the `wasi:http/service` world of the 0.3 release
candidate needs four things from the polyfill. Those are the value types
`stream<u8>`, `future<result<option<trailers>, error-code>>`, and
`future<result<_, error-code>>`, then resources, an `async` export, and `async`
imports. All of those are in this design or the two before it. The binding layer
of the Rust toolchain links `task.cancel` in every `async` export and
`subtask.cancel` in every awaited import. Both are accepted at translation here,
so the guest instantiates and its handler runs its paths that never cancel. The
host side of such a fixture supplies the `wasi:http/types` interface. It creates
request bodies as host streams and reads response bodies through host consumers,
through the host surface of this design.

## User Stories

A developer serves an HTTP request to a component in a browser page.

> The developer builds a request whose body is a `StreamReader<u8>` over a
> producer that awaits chunks of a fetch response, and calls the handler. The
> guest reads the body in turns while the page stays responsive. The guest
> returns a response whose body the developer pipes to a consumer that feeds a
> `ReadableStream`, and the trailers future resolves after the last chunk.

A developer returns a value to a guest later.

> The developer's host function returns a `FutureReader<T>` made from an `async`
> block. The guest reads the future asynchronously, joins the end to its set,
> and waits. When the block resolves, a turn lowers the value into the guest's
> buffer and delivers the future read event.

A component author streams results between two components.

> The author's component calls a sibling's `async` export that returns a
> `stream<record>`. The readable end moves into the author's table. The author
> reads it in chunks of one hundred, each read returning at once while the
> sibling's pending write still has items, and blocking otherwise.

A guest author probes readiness.

> The author starts a zero-length read and waits on the end. The event arrives
> when the writer has data pending. The author then reads a full buffer, and
> handles the case where that read blocks anyway.

A host author closes a stream early.

> The author calls `close` on a reader after enough data arrived. The guest's
> next write completes with the dropped result, and the guest drops its writable
> end.

A contributor lands the browser fixture built by a real toolchain.

> The contributor finds the fixture's `task.cancel` import accepted and the
> handler running. The contributor records the cancellation directives of the
> fixture as deferred features with that reason.

## Test Cases

The type projection accepts streams and futures. A host reads
`ValueType::Stream` and `ValueType::Future` with the payload type on an import
and on an export. A repository test proves it on a hand-written component, and
`validate-no-stream-char.wast` proves the refusal of `stream<char>`.

`stream.new` and `future.new` create two ends. The `i64` holds the readable
index in its low half and the writable index in its high half, both from the
instance's allocator. The components of `streams.wast` and `futures.wast` that
define the two built-ins instantiate, and a repository test reads the two
indices.

A read and a write pair up. `streams.wast`, `intra-streams.wast`,
`stream-zero-ops.wast`, `zero-length.wast`, `partial-stream-copies.wast` in both
corpora, and `closed-stream.wast` pass whole. A repository test proves that a
pending read keeps accepting writes until its event is delivered and that the
event reports the total.

A number payload copies as bytes. A repository test writes a `stream<u8>` of one
mebibyte between two components and reads the same bytes back through one read.
A second test proves that a `stream<record>` copies through values and that a
`stream<own<R>>` moves the handles between the two tables.

A read and a write from one instance follow the number rule.
`same-component-stream-future.wast`, `intra-futures.wast`, and
`intra-streams.wast` pass whole, with the same-instance trap for a non-number
payload and success for a number payload.

Futures complete once. `futures.wast`, `future-read.wast`,
`futures-must-write.wast` in both corpora, `futures-must-write2.wast`,
`cross-task-future.wast`, `empty-wait.wast`, and `wait-during-callback.wast`
pass whole. `trap-if-done.wast` in both corpora passes whole, with every
after-done trap for both kinds and both directions.

A copy cancel reports progress. `cancel-stream.wast`, `async-builtins.wast`,
`future-cancel-read-dropped.wast`, `future-cancel-write-completed.wast`,
`future-cancel-write-dropped.wast`, and `stream-cancel-finished-op.wast` pass
whole. A repository test proves the no-copy-pending trap.

Dropping an end behaves as the reference states. `drop-stream.wast`,
`future-drop-writable-after-notified-drop.wast`, and the busy-stream directive
of `builtin-trap-poisons-instance.wast` pass. A repository test proves that
dropping the second end frees the shared record and its index.

A readable end crosses the boundary. `passing-resources.wast`,
`drop-cross-task-borrow.wast`, `trap-if-transfer-in-waitable-set.wast` in both
corpora, and `waitable-set-stale-entry.wast` pass whole. A repository test
proves the transfer between two composed components through the two intrinsics,
the lift-after-done trap, and that the writable end stays in the creating
instance.

The copy budget holds. `streams-massive-send.wast` passes whole. A repository
test proves that a synchronous call with a list result over the budget fails
with the budget cause, and that a result under it succeeds.

Synchronous copies block under the four rules. `future-read.wast` proves the
cannot-block and deadlock causes. `sync-and-async-waitable.wast` and the first
four directives of `cancel-sync-and-waitable.wast` prove the waitable cause. A
repository test proves the stack-switch cause for a synchronous read against a
pending host producer. A second test proves that a synchronous read completes
when a callback task of another instance writes inside the nested turn.

A host writes a stream and a future. Repository tests prove five facts. A
`StreamReader<u8>` over a producer feeds a guest's asynchronous reads across
several turns. Items beyond the guest's capacity wait for the next read. A
zero-length read reaches the producer with no capacity. A `FutureReader<T>` over
an `async` block resolves a guest's read. A producer error fails the guest's
built-in. In the browser the producer awaits a JavaScript promise.

A host reads a stream and a future. Repository tests prove four facts. A guest's
writes reach a piped `StreamConsumer` inside turns. A consumer's pending result
delays the writer's completion. A guest cancel reaches the consumer with
`finish` set. A `FutureConsumer` receives the guest's one value.

The lifecycle rule holds. Repository tests prove four facts. `close` completes a
guest's pending write with the dropped result. A reader dropped without close
leaves a guest write pending until the store drops. A guard closes on drop
inside a poll. Dropping the store drops every record and every producer and
consumer.

The untyped values cross. Repository tests prove four facts. `Func::call`
returns `Val::Stream` for a stream result. `func_new` receives `Val::Future` for
a future parameter. Conversion to a typed reader checks the payload type.
Lowering an `Any` value into a guest of another payload type fails.

The cancel built-ins fail only at the call. A component that imports
`task.cancel` and `subtask.cancel` instantiates, and a call to either fails with
`Error::Unsupported`. The `subtask.cancel` component of `task-builtins.wast`
instantiates. The lines of every cancellation file change as The Corpus states.

The HTTP handler runs. Once the fixture built from the `wasi:http/service` world
exists, it instantiates and its handler answers a request with a body stream and
a trailers future. Its cancellation directives stay deferred for cancellation.
Until it exists, a repository test proves the same shapes on a hand-written
component whose export takes a `stream<u8>` and returns a
`future<result<_, u32>>`.

The corpus lines are removed. The expected-failure list loses every line the
owned files and directives above account for, and no other line. The progress
summary shows the two `async` rows with the new counts, the same on both
targets.

Every facet above holds on both targets. The native run and the browser run
report the same pass and failure results for every named file and every
repository test.

## References

- [PDD006], component parsing, whose translator emits the built-in trampolines.
- [PDD008], the canonical ABI and host functions, whose typed host function
  carries a reader.
- [PDD010], the typed call surface.
- [PDD014], borrow lifetime tracking.
- [PDD015], component composition, whose transfer rule the readable end follows.
- [PDD016], the conformance suite.
- [PDD018], the concurrency runtime model, whose reserved handle kinds, waitable
  state, host tasks, boundary context, and store-drop rule this design fills.
- [PDD019], tasks and the callback export, whose waitable sets deliver the copy
  events.
- [PDD020], subtasks and the async import, whose blocking rules a synchronous
  copy uses.
- [Concurrency], the Concurrency explainer, and its sections on [streams and
  futures][Streams and Futures] and [stream readiness][Stream Readiness].
- [CanonicalABI], the Canonical ABI explainer, and its sections on [buffer
  state][CanonicalABI – buffers], [stream state][CanonicalABI – stream state],
  [future state][CanonicalABI – future state],
  [`{stream,future}.new`][CanonicalABI – new],
  [`stream.{read,write}`][CanonicalABI – stream copy],
  [`future.{read,write}`][CanonicalABI – future copy],
  [`{stream,future}.cancel-{read,write}`][CanonicalABI – cancel], and
  [`{stream,future}.drop-{readable,writable}`][CanonicalABI – drop].
- [`definitions.py`], the executable reference for `SharedStreamImpl`,
  `SharedFutureImpl`, `CopyEnd`, `lift_async_value`, `lower_stream`,
  `stream_copy`, `future_copy`, `cancel_copy`, and `drop`.
- [Explainer – canonical definitions], the fourteen built-ins and their
  validation, and [Explainer – stream and future types], the `borrow` and `char`
  rules.
- [Wasmtime], the reference implementation at `v49.0.0-rc.1`: its [streams and
  futures][Wasmtime streams], its [buffers][Wasmtime buffers], its [untyped
  values][Wasmtime any], its [handle table][Wasmtime handle table], its [copy
  budget][Wasmtime budget], its [flat payload rule][Wasmtime flat], and its
  [trap messages][Wasmtime traps].
- The Wasmtime API for [`StreamReader`], [`FutureReader`], [`StreamProducer`],
  [`StreamConsumer`], [`FutureProducer`], [`FutureConsumer`], [`StreamAny`], and
  [`FutureAny`].
- [wasi-http 0.3], the WIT of the `wasi:http/service` world.
- [wit-bindgen], whose Rust guest runtime links `task.cancel` in every `async`
  export.
- The [Component Model test corpus] and the [Wasmtime component tests].

[PDD006]: ./PDD006%20Component%20Parsing.md
[PDD008]: ./PDD008%20Canonical%20ABI%20and%20Host%20Functions.md
[PDD010]: ./PDD010%20Pre-wasip3%20Public%20API.md
[PDD014]: ./PDD014%20Borrow%20Lifetime%20Tracking.md
[PDD015]: ./PDD015%20Component%20Composition.md
[PDD016]: ./PDD016%20Conformance%20Suite.md
[PDD018]: ./PDD018%20Concurrency%20Runtime%20Model.md
[PDD019]: ./PDD019%20Tasks%20and%20the%20Callback%20Async%20Export.md
[PDD020]: ./PDD020%20Subtasks%20and%20the%20Async%20Import.md
[Concurrency]:
  https://github.com/WebAssembly/component-model/blob/main/design/mvp/Concurrency.md
[Streams and Futures]:
  https://github.com/WebAssembly/component-model/blob/main/design/mvp/Concurrency.md#streams-and-futures
[Stream Readiness]:
  https://github.com/WebAssembly/component-model/blob/main/design/mvp/Concurrency.md#stream-readiness
[CanonicalABI]:
  https://github.com/WebAssembly/component-model/blob/main/design/mvp/CanonicalABI.md
[CanonicalABI – buffers]:
  https://github.com/WebAssembly/component-model/blob/main/design/mvp/CanonicalABI.md#buffer-state
[CanonicalABI – stream state]:
  https://github.com/WebAssembly/component-model/blob/main/design/mvp/CanonicalABI.md#stream-state
[CanonicalABI – future state]:
  https://github.com/WebAssembly/component-model/blob/main/design/mvp/CanonicalABI.md#future-state
[CanonicalABI – new]:
  https://github.com/WebAssembly/component-model/blob/main/design/mvp/CanonicalABI.md#-canon-streamfuturenew
[CanonicalABI – stream copy]:
  https://github.com/WebAssembly/component-model/blob/main/design/mvp/CanonicalABI.md#-canon-streamreadwrite
[CanonicalABI – future copy]:
  https://github.com/WebAssembly/component-model/blob/main/design/mvp/CanonicalABI.md#-canon-futurereadwrite
[CanonicalABI – cancel]:
  https://github.com/WebAssembly/component-model/blob/main/design/mvp/CanonicalABI.md#-canon-streamfuturecancel-readwrite
[CanonicalABI – drop]:
  https://github.com/WebAssembly/component-model/blob/main/design/mvp/CanonicalABI.md#-canon-streamfuturedrop-readablewritable
[`definitions.py`]:
  https://github.com/WebAssembly/component-model/blob/main/design/mvp/canonical-abi/definitions.py
[Explainer – canonical definitions]:
  https://github.com/WebAssembly/component-model/blob/main/design/mvp/Explainer.md#canonical-definitions
[Explainer – stream and future types]:
  https://github.com/WebAssembly/component-model/blob/main/design/mvp/Explainer.md#asynchronous-value-types
[Wasmtime]: https://github.com/bytecodealliance/wasmtime
[Wasmtime streams]:
  https://github.com/bytecodealliance/wasmtime/blob/v49.0.0-rc.1/crates/wasmtime/src/runtime/component/concurrent/futures_and_streams.rs
[Wasmtime buffers]:
  https://github.com/bytecodealliance/wasmtime/blob/v49.0.0-rc.1/crates/wasmtime/src/runtime/component/concurrent/futures_and_streams/buffers.rs
[Wasmtime any]:
  https://github.com/bytecodealliance/wasmtime/blob/v49.0.0-rc.1/crates/wasmtime/src/runtime/component/concurrent/future_stream_any.rs
[Wasmtime handle table]:
  https://github.com/bytecodealliance/wasmtime/blob/v49.0.0-rc.1/crates/wasmtime/src/runtime/vm/component/handle_table.rs
[Wasmtime budget]:
  https://github.com/bytecodealliance/wasmtime/blob/v49.0.0-rc.1/crates/wasmtime/src/runtime/component/store.rs
[Wasmtime flat]:
  https://github.com/bytecodealliance/wasmtime/blob/v49.0.0-rc.1/crates/cranelift/src/compiler/component.rs
[Wasmtime traps]:
  https://github.com/bytecodealliance/wasmtime/blob/v49.0.0-rc.1/crates/environ/src/trap_encoding.rs
[`StreamReader`]:
  https://docs.wasmtime.dev/api/wasmtime/component/struct.StreamReader.html
[`FutureReader`]:
  https://docs.wasmtime.dev/api/wasmtime/component/struct.FutureReader.html
[`StreamProducer`]:
  https://docs.wasmtime.dev/api/wasmtime/component/trait.StreamProducer.html
[`StreamConsumer`]:
  https://docs.wasmtime.dev/api/wasmtime/component/trait.StreamConsumer.html
[`FutureProducer`]:
  https://docs.wasmtime.dev/api/wasmtime/component/trait.FutureProducer.html
[`FutureConsumer`]:
  https://docs.wasmtime.dev/api/wasmtime/component/trait.FutureConsumer.html
[`StreamAny`]:
  https://docs.wasmtime.dev/api/wasmtime/component/struct.StreamAny.html
[`FutureAny`]:
  https://docs.wasmtime.dev/api/wasmtime/component/struct.FutureAny.html
[wasi-http 0.3]:
  https://github.com/WebAssembly/wasi-http/tree/main/wit-0.3.0-draft
[wit-bindgen]:
  https://github.com/bytecodealliance/wit-bindgen/blob/main/crates/guest-rust/src/rt/async_support.rs
[Component Model test corpus]:
  https://github.com/WebAssembly/component-model/tree/main/test/async
[Wasmtime component tests]:
  https://github.com/bytecodealliance/wasmtime/tree/main/tests/misc_testsuite/component-model/async
