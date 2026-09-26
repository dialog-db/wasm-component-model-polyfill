# Cancellation, Error Contexts, and the Poisoned Store

[PDD018] through [PDD022] built the concurrency runtime: tasks, subtasks,
streams, futures, and threads. Each of them left the same three things out. This
document designs them:

- Cancellation. A caller asks a subtask to stop early with `subtask.cancel`, and
  the callee confirms with `task.cancel`.
- Error contexts. An error context is an opaque value that carries a debug
  message from one component to another.
- The poisoned store. After a trap, the store refuses to run guest code again.

The three belong together because each one decides what happens to a call that
does not end with a result. The design also states, for every failure, which
host call reports it.

The design follows the [Concurrency explainer][Concurrency], the
[Explainer][Explainer – invariants], and the Python reference in
[`definitions.py`] at the commit the conformance corpus of [PDD016] is vendored
from. Where the reference leaves a choice to the host, the design makes the
choice [Wasmtime] makes at `v49.0.0-rc.1`. In one place the design follows
Wasmtime and wit-bindgen over the reference. There, one guest binary must behave
the same in the polyfill and in Wasmtime. Each departure states its reason.

Six terms recur:

- A trap is a failure that ends guest execution. The reference raises one with
  `trap()`. A failure of core code is a trap too.
- A poisoned store is a store in which a trap happened. It runs no more guest
  code.
- A driver is a host future that polls the scheduler, as [PDD018] defines it.
- A cancellation request is the mark that `subtask.cancel` puts on the callee's
  task.
- Delivery is the moment the callee learns of the request.
- An error-context record is the store's record of one error context. A handle
  in an instance's table names it.

## Goals

- A trap poisons the store. The triggers are the triggers of Wasmtime, plus the
  failure of a host `async` function.
- A poisoned store runs no more guest code. Every host entry into a guest fails
  with Wasmtime's cannot-enter trap. Host work that touches no guest still runs.
- The first trap ends the driver that is polling, with that trap. No failure is
  lost, and no failure waits for a caller that already left.
- `subtask.cancel` requests cancellation of a guest callee or a host callee, as
  the reference and Wasmtime state it.
- A callee learns of the request at the entry gate, in the callback loop, or in
  a built-in that carries the `cancellable` immediate. `task.cancel` confirms
  it.
- The three `error-context` built-ins run. An error context crosses between
  components as a value, and the host sees it through Wasmtime's types.
- The store can cap the number of its live records. The conformance harness uses
  the cap to prove that a cancelled subtask leaves nothing behind.
- Every trap the feature adds is a structured error cause with Wasmtime's
  message, so the conformance corpora match it by substring.
- The behavior is the same on both targets.

## Non-goals

- Deadlock detection. [PDD019], [PDD020], and [PDD022] define the deadlock, the
  cannot-block, and the stack-switch causes, and this design keeps them.
- Host-side cancellation of a task that `call_concurrent` started. Dropping the
  call's future cancels nothing, as [PDD018] states. Wasmtime offers no such
  cancellation at this version.
- A trap contained inside a group of instances, or a guest task ended by force.
  The Component Model defers both to a later feature it names the blast zone
  ([blast zones]). A trap ends the whole store until then.
- A host API that reads, creates, or drops an error context. Wasmtime offers
  none at this version.
- A public query that answers whether a store is poisoned. Wasmtime keeps its
  flag internal.
- Delivery of a cancellation to a stackful task through a built-in that has no
  `cancellable` immediate. The reference states no such delivery.

## Facts This Design Rests On

Each fact below was read from the cited source.

- The Explainer gives each component instance a lockdown state that is set upon
  a trap and "implicitly checked at every execution step". After a trap, no
  guest code runs ([Explainer – invariants]).
- The Canonical ABI explainer states that a trap will "tear down the whole
  store" ([CanonicalABI – canon lift]).
- The Component Model defers two things to a post-MVP feature, the blast zone: a
  trap that a parent contains, and the forced end of a thread ([blast zones],
  [Concurrency – cancellation]).
- Wasmtime keeps one trapped flag per store ([Wasmtime trapped]). Every failed
  core call sets it ([Wasmtime core trap]). So does every failed `Func::call` or
  `TypedFunc::call` after the arity check ([Wasmtime call trap]).
- Wasmtime refuses three host entries once the flag is set: `Func::call`
  ([Wasmtime call enter]), the prepared concurrent call ([Wasmtime prepared
  enter]), and a host drop of a guest-defined resource ([Wasmtime drop enter]).
  The message is "cannot enter component instance".
- In Wasmtime, a failure of a work item or a host future ends the event loop.
  The whole `run_concurrent` returns that failure ([Wasmtime event loop]).
- The reference delivers a cancellation request in two places only: at the entry
  gate, and in the callback loop ([`definitions.py`], `Task`, `canon_lift`,
  `wait_from_callback`).
- The reference removed the `cancellable` immediate on 2026-09-04 ([spec #716]).
  Wasmtime 49 still honors it ([Wasmtime cancellable wait]). The C generator of
  wit-bindgen still emits the cancellable thread built-ins ([wit-bindgen C]).
- Wasmtime ends a host callee's future at `subtask.cancel` through an abort
  handle. The event loop drops the future at its next poll ([Wasmtime subtask
  cancel]).
- The reference lets the host keep an error context's debug message or replace
  it with an empty string ([`definitions.py`], `canon_error_context_new`).
  Wasmtime keeps it ([Wasmtime error context]).
- Wasmtime counts the guest handles of each error context across the store. A
  transfer between components adds one ([Wasmtime error-context transfer]). A
  lower from the host adds a handle and no count.
- Wasmtime's host types for an error context carry no operation. The doc of
  `ErrorContextAny` states that the host cannot drop one yet ([Wasmtime
  error-context any]).
- Wasmtime's wast runner provides `wasmtime.set-max-table-capacity`, which caps
  the store's concurrent table ([Wasmtime runner]). The default cap is 1,000,000
  entries, and a full table fails with "resource table has no free keys"
  ([Wasmtime resource table]).

## The Poisoned Store

### What Poisons the Store

A trap poisons the store. These failures poison it:

- A failure of guest core code, including a failure of a host function that
  guest code called.
- A failure of a built-in, a lift, a lower, or a fused adapter while a guest
  call runs.
- A failure of a core `start` function or an initializer during instantiation.
- A failure of a resource destructor.
- A host `Val` of the wrong type that `Func::call` finds while it lowers the
  arguments.
- A host `async` function whose future completes with an error.

The first five are Wasmtime's triggers. A mistyped host `Val` poisons the store
in Wasmtime because the lowering runs after the entry check. The polyfill
matches it.

The last trigger is the polyfill's own. In Wasmtime, a failed host future ends
the event loop but does not set the flag. Here the failure is a trap of the
guest task that made the call. That task can never resolve its subtask, which
the reference does not permit. So one rule holds for every failure: a trap
poisons.

These failures do not poison, because the polyfill raises each one before any
guest state changes:

- An arity mismatch of `Func::call`.
- A driver entered inside a turn, with the recursive-driver cause.
- An accessor used outside a poll, with the store-not-in-poll cause.
- A value from another store, with the wrong-store cause.
- A link error.

### What a Poisoned Store Refuses

A poisoned store runs no more guest code. That is the lockdown of the Explainer,
applied to the whole store as the Canonical ABI states it. These entries fail
with the cannot-enter trap, "cannot enter component instance":

- `Func::call` and `TypedFunc::call`.
- `Func::call_concurrent` and `TypedFunc::call_concurrent`.
- Every instantiation into the store, through `Linker::instantiate` or a core
  module's instantiation.
- `Store::resource_drop` of a resource that a guest defines.

Wasmtime refuses the first three and lets instantiation run. Instantiation runs
guest `start` functions, so the polyfill refuses it too. This is a departure
from Wasmtime in favor of the reference.

These entries still work, because no guest code runs:

- `Store::resource_drop` of a resource that the host defines.
- `Store::run_concurrent`, when its closure does only host work. An entry into a
  guest from inside the closure fails with the cannot-enter trap.
- Reads and writes of the store's host data.
- Dropping the store.

At the moment the store is poisoned, it discards two kinds of work:

- Every queued guest work item, such as a callback, a start item, or a ready
  thread.
- Every pending host future. That includes host `async` functions, stream and
  future producers, and consumers. Each future is dropped.

The task and subtask records stay until the store drops, as [PDD018] states for
every record. A later driver meets no stale work, so it fails only for an entry
it makes itself. Wasmtime keeps its queued items and host futures, and a later
`run_concurrent` runs them. The polyfill discards them, because the reference
permits no guest code after a trap.

```text
fn poison(store, trap):
    store.poisoned = true
    store.work_items.clear()         // no guest code runs again
    store.host_tasks.clear()         // each host future is dropped here
    return trap                      // the polling driver reports it

fn enter_guest(store):
    if store.poisoned:
        return Err(CannotEnter)
```

### Where a Failure Goes

The first trap ends the driver that is polling, with that trap. A driver is
`Func::call`, `TypedFunc::call`, an instantiation, or `Store::run_concurrent`.
The rule is Wasmtime's. It holds whichever task the trap belongs to:

- A trap in the call's own task ends that call.
- A trap in work that a task left after it resolved ends the driver whose turn
  runs that work. [PDD019] states the same rule for a callback task.
- A trap inside `run_concurrent` ends the whole `run_concurrent`, and its
  closure is dropped with every call future inside it.
- A trap in a thread that outlives its task's host call ends the driver whose
  turn runs that thread.
- A failed host `async` function ends the driver whose turn polls it.

The rule has one outcome for every shape. The trap is never held for a caller
that already has its result, so no failure is lost. The store is poisoned in the
same step, so the next driver fails with the cannot-enter trap.

The polyfill carries no per-task channel for failures. A caller that already
returned never learns of a later trap in its task, and the next driver reports
it.

A user story shows the rule.

> A host runs two concurrent calls of one export inside `run_concurrent`. The
> first call's guest code traps on an out-of-bounds load. `run_concurrent`
> returns that trap, and both call futures are gone. The host logs the trap and
> calls the export again. The call fails with "cannot enter component instance".
> The host drops the store and builds a new one.

## Cancellation

Cancellation is cooperative. A caller asks, and the callee decides when to stop.
Nothing ends a guest task by force, which is the rule of the Concurrency
explainer ([Concurrency – cancellation]).

### The Request

`subtask.cancel` takes a subtask index and an `async` option. It follows the
reference ([`definitions.py`], `canon_subtask_cancel`):

```text
fn subtask_cancel(async_, index) -> status:
    trap if the instance's may-leave flag is clear
    subtask = the subtask at index            // trap if it is not a subtask
    trap if its resolution was delivered      // "called after terminal status delivered"
    trap if cancellation was requested        // the second cancel of one subtask
    trap if it is in a waitable set           // "cannot be used synchronously ..."
    if not subtask.resolved():
        subtask.cancellation_requested = true
        subtask.callee.request_cancellation()
        if not subtask.resolved():
            if async_: give way once          // the cooperative yield
            else:      block until resolved   // under the existing blocking rules
        if not subtask.resolved():
            return BLOCKED                    // 0xffff_ffff
    deliver the resolution
    return subtask.state
```

A callee in the `STARTING` state has not started. The callee's task never runs.
The subtask resolves to `CANCELLED_BEFORE_STARTED`, and its record leaves the
store when the caller drops it.

A started guest callee receives the request as the next section states. A
started host callee is the subject of The Host Callee.

An asynchronous `subtask.cancel` gives way once when the callee has not
resolved. The give-way is the one `thread.yield` makes under [PDD019], so ready
work, including the callee, can run. If the callee still has not resolved, the
built-in returns `BLOCKED`. The caller then waits for the subtask event.

A synchronous `subtask.cancel` blocks until the callee resolves. It blocks as
every blocking built-in blocks: through the provider of [PDD022], or in a nested
turn under the rules of [PDD020]. A block that cannot progress fails with the
cause [PDD022] states.

The resolution is `RETURNED` when the callee returned first, and
`CANCELLED_BEFORE_RETURNED` when it confirmed the cancellation. Delivery of
either state releases the handles the caller lent, as [PDD020] states.

### Delivery to a Guest Callee

The request marks the callee's task as pending-cancel. The task learns of it at
the first of these points:

- At the entry gate. A task that waits at the gate is cancelled there, before
  its thread runs ([`definitions.py`], `enter_implicit_thread`).
- In the callback loop. The callback receives the task-cancelled event (6)
  instead of the next event. That happens after a wait, after a yield, or at
  once when the task is waiting. The instance's exclusive lock must be free,
  because the callback runs under it ([`definitions.py`], `wait_from_callback`).
- In a built-in that carries the `cancellable` immediate, as the next section
  states.

A task learns of a request once. The first delivery moves it to
`cancel-delivered`. A stackful task that never calls a cancellable built-in is
never told, and the cancel waits until the task resolves on its own. The
reference states this, and Wasmtime does the same.

When `subtask.cancel` requests cancellation of a started callee, it wakes one of
the callee's threads if one can take the request at once. The candidates are a
callback task waiting in its loop, a thread in a cancellable
`waitable-set.wait`, and a thread in a cancellable yield. Wasmtime wakes the
first such thread it finds and gives way to it ([Wasmtime subtask cancel]). The
polyfill does the same.

### The Cancellable Immediate

The reference removed the `cancellable` immediate from the built-ins on
2026-09-04 ([spec #716]). Wasmtime 49 still reads it and honors it. The C
generator of wit-bindgen still emits `[cancellable][thread-suspend]`,
`[cancellable][thread-yield]`, and the four cancellable switch built-ins. A C
guest that uses them runs under Wasmtime today.

The polyfill honors the immediate as Wasmtime 49 does. Ecosystem coherence wins
over the reference here: one guest binary must behave the same in the polyfill
and in Wasmtime. The translator reads the immediate and keeps it. It no longer
drops it. When the toolchain and Wasmtime drop the immediate, this section goes
with them.

The rules follow Wasmtime ([Wasmtime cancellable wait], [Wasmtime cancellable
suspend]):

- Every cancellable built-in first takes a pending request, if one exists, and
  returns the cancelled result at once. `waitable-set.wait` and
  `waitable-set.poll` return the task-cancelled event (6). The thread built-ins
  return 1.
- A cancellable `waitable-set.wait` that blocks can be woken by
  `subtask.cancel`. It returns the task-cancelled event.
- A cancellable yield, from `thread.yield` or from a promote that yields, is run
  first by `subtask.cancel`. It returns 1.
- A cancellable suspend, from `thread.suspend` or from a switch that suspends,
  is not woken by `subtask.cancel`. When another thread resumes it, it returns 1
  if the request is still pending.
- A built-in without the immediate never takes a request. Its result does not
  change.

A task that takes a request through a cancellable built-in is in
`cancel-delivered`, as it is after any other delivery.

Without a provider, a cancellable block waits in a nested turn. Its readiness
condition includes a pending request, so a request made by work inside the
nested turn wakes it. A request from a frame below it cannot run, and the block
fails with the stack-switch cause, as [PDD022] states for every such block.

### The Confirmation

`task.cancel` resolves the current task as cancelled. It follows the reference
([`definitions.py`], `Task.cancel`) and uses Wasmtime's messages:

- The may-leave flag must be set, or the built-in fails with the cannot-leave
  trap.
- The task must be in `cancel-delivered`. Otherwise the built-in fails with
  "`task.cancel` called by task which has not been cancelled". A task that is
  not lifted `async` can never receive a request, so it fails with the same
  message.
- The task must not have resolved. Otherwise the built-in fails with
  "`task.return` or `task.cancel` called more than once for current task".
- The task's borrow count must be zero, which is the scope-exit rule of
  [PDD014].

The caller's subtask then resolves to `CANCELLED_BEFORE_RETURNED`. A task in
`cancel-delivered` can still call `task.return` instead. Its caller then sees
`RETURNED`. The task's threads keep running after `task.cancel`, and the task
lives until its last thread ends, as [PDD022] states.

### The Host Callee

A host `async` function is cancelled by dropping its future. The host sees only
the drop. The polyfill gives the host no signal before the drop and no way to
return a value after it, as Wasmtime gives none.

`subtask.cancel` marks the host task as aborted and returns. The scheduler drops
the future at its next poll of the host tasks, and the subtask resolves:

- `CANCELLED_BEFORE_RETURNED`, when the future had not completed.
- `RETURNED`, when the future completed before the abort. The result lowers as
  usual.

A synchronous `subtask.cancel` blocks until that poll. An asynchronous one
returns `BLOCKED`, and the caller waits for the subtask event. The borrows lent
to the call come back when the resolution is delivered.

The drop happens in a turn and not inside the built-in. A future's `Drop` can
reach the store through its accessor. So `Drop` runs where host code is allowed
to run: inside a turn, with no guest frame between it and the driver. Wasmtime
makes the same choice through its abort handle.

A second `subtask.cancel` of a host callee whose resolution was delivered fails
with "`subtask.cancel` called after terminal status delivered". The Wasmtime
corpus expects that message.

### The Two Record States

With this design, the two cancel states of the task record of [PDD018] are
reached: `pending-cancel` by a request, and `cancel-delivered` by a delivery.
The two cancelled states of the subtask record are reached only through
cancellation.

## Error Contexts

### The Record and Its Handles

An error context is one store-wide record. The record holds the debug message
and a count of the guest handles that name it. A handle in an instance's table
has the error-context kind that [PDD018] reserved. The record leaves the store
when its count reaches zero and no host value holds it.

The count follows Wasmtime ([Wasmtime error-context transfer]):

- `error-context.new` creates the record with a count of one.
- A transfer to another instance adds a handle and adds one to the count.
- `error-context.drop` removes a handle and subtracts one.
- A count past `u32::MAX` fails with Wasmtime's "reference count overflow".

A lift to the host marks the record as host-held. A host-held record stays until
the store drops, because the host cannot drop an error context. A lower from the
host adds a handle and adds one to the count. Wasmtime adds no count there, so a
guest drop can free a record that the host still holds. The reference keeps the
value alive while anything refers to it. The polyfill follows the reference.

### The Three Built-ins

Each built-in first fails with the cannot-leave trap when the instance's
may-leave flag is clear, as the reference states for all three
([`definitions.py`], `canon_error_context_new`).

- `error-context.new` takes the canon options `memory` and `string-encoding`. It
  reads the debug message from guest memory in that encoding and keeps it
  exactly as written. It returns the new handle. A message out of bounds fails
  with the string bounds checks of [PDD008].
- `error-context.debug-message` takes the options `memory`, `realloc`, and
  `string-encoding`. It writes the message into guest memory through `realloc`.
  It stores the pointer and the length of the string at the address the guest
  passes.
- `error-context.drop` removes the handle and updates the count.

`error-context.debug-message` first makes sure that the eight bytes at the
guest's address are in bounds, and only then calls `realloc`. An address out of
bounds fails with Wasmtime's "invalid debug message pointer: out of bounds". The
reference calls `realloc` first and fails on the store. The order differs, but
no guest can observe it: the trap poisons the store before the realloc's effect
can be read.

In `error-context.debug-message` and `error-context.drop`, a handle of another
kind fails with Wasmtime's "handle is not an error-context".

### Transfer and the Value Type

`error-context` is a value type. It flattens to one `i32` and takes four bytes
in memory with an alignment of four. It appears anywhere a value appears:

- In a parameter or a result.
- Inside a record, a tuple, a variant, an option, a result, or a list.
- As the payload of a stream or a future, through the boundary context of
  [PDD021].

A lift reads the handle from the source instance and fails when the handle is
not an error context. A lift does not remove the source handle. An error context
is copied between components, not moved. A lower adds a handle to the
destination instance and adds one to the count.

### The Host Surface

The host sees an error context through Wasmtime's names:

- `Val::ErrorContext(ErrorContextAny)` in the untyped values.
- `ErrorContext` in the typed entries. It converts to and from a `Val` and lifts
  and lowers as a typed value.

Neither type has an operation. The host cannot read the debug message, create an
error context, or drop one. Wasmtime marks the same gap in its source and plans
to fill it ([Wasmtime error-context any]). The polyfill matches the capability
of Wasmtime and does not go past it. When Wasmtime adds the operations, the
polyfill adds them under the same names.

## The Record Cap

The store can cap the number of its live records. The records it counts are
tasks, subtasks, threads, host tasks, waitable sets, the shared records of
streams and futures, and error-context records. A new record past the cap fails
with Wasmtime's "resource table has no free keys". A cap lower than the current
count evicts nothing, and only new records fail.

The default cap is 1,000,000 records, which is Wasmtime's default. Only the
store's internal API changes the cap. No public method does. The conformance
harness uses the internal API to register the item the Wasmtime runner provides:
`wasmtime.set-max-table-capacity`. With it,
`cancel-starting-subtask-does-not-leak.wast` proves what it states. The file
cancels and drops a held-back call 1,000 times under a cap of 100. A leaked
record fails the file.

## Translation

The translator accepts the three `error-context` built-ins and the
`error-context` value type on every target. One gate of `EngineConfig` decides
them, as in Wasmtime: `wasm_component_model_error_context`. The gate stays off
by default.

`task.cancel` and `subtask.cancel` run. [PDD021] let the translator accept them
and fail each one when a guest called it. That failure is gone.

The translator keeps the `cancellable` immediate of `waitable-set.wait`,
`waitable-set.poll`, and the thread built-ins, and passes it to each built-in.

## Error Model Growth

- The cannot-enter cause, "cannot enter component instance", joins the causes of
  `Error::Task`, next to the cannot-leave cause.
- `task.cancel` adds "`task.cancel` called by task which has not been
  cancelled". A second resolution keeps the existing message "`task.return` or
  `task.cancel` called more than once for current task".
- `subtask.cancel` adds "`subtask.cancel` called after terminal status
  delivered". A second cancel of a subtask whose resolution was not delivered
  fails with "`subtask.cancel` called twice for the same subtask". The reference
  traps there, and Wasmtime has no message for it, so the message is the
  polyfill's own. A cancel of a subtask in a waitable set fails with the
  existing "waitable cannot be used synchronously while added to a waitable
  set".
- The record cap adds "resource table has no free keys".
- Error contexts add "invalid debug message pointer: out of bounds", "handle is
  not an error-context", and "reference count overflow".

Each is a structured cause of `wcmp::Error`.

## Revisions to Earlier Designs

This design revises three statements of [PDD018]:

- A host `async` function whose future fails traps. It ends the driver whose
  turn polls it, and it poisons the store.
- A host task can end through cancellation. The scheduler drops its future at
  the next poll of the host tasks.
- The cancel states of the task record and the cancelled states of the subtask
  record are reached through cancellation.

This design revises one statement of [PDD019]:

- `task.cancel` runs, and the task-cancelled event is delivered to a callback.

This design revises three statements of [PDD020]:

- A failed call poisons the store. The two cancelled states of the subtask
  record are reached only through cancellation, never through a failure.
- `CannotEnterComponent` is raised by a poisoned store.
- `subtask.cancel` runs.

This design revises two statements of [PDD021]:

- `task.cancel` and `subtask.cancel` run when a guest calls them.
- The error-context built-ins and value type are accepted.

This design revises one statement of [PDD022]:

- The `cancellable` immediate of the thread built-ins is honored, as Wasmtime 49
  honors it.

## Target Differences

None. The poisoned store, cancellation, and error contexts behave the same on
both targets. On both, a trap of core code reaches the polyfill as a failure of
the runtime layer. The drop of a host future happens in a turn on both.
`builtin-trap-poisons-instance.wast:9` stays in the web list, for the browser's
wording of the `unreachable` trap only.

## The Corpus

The directives this design owns, in the Component Model corpus:

- Cancellation: `big-interleaving-test.wast` (1623, 1651, 1664, 1675),
  `cancel-and-exclusive-lock.wast:196`, `cancel-delivery.wast:278`,
  `cancel-subtask.wast:217`, `reentrance.wast:548` and `:891`,
  `trap-if-block-and-sync.wast:352`, and `trap-if-sync-and-waitable-set.wast`
  (325, 327).
- The poisoned store: `builtin-trap-poisons-instance.wast` (10, 39).

In the Wasmtime corpus:

- Cancellation: `cancel-host.wast` (80, 167, 256, 385, 476),
  `cancel-sibling-subtask.wast` (146, 150),
  `cancel-starting-subtask-does-not-leak.wast` (9, 103),
  `cancel-sync-and-waitable.wast:301`, and `yield-when-cancelled.wast:97`.
- Error contexts: `error-context.wast` (5, 16, 30, 39, 84, 85, 86), and
  `error-context-trap-in-post-return.wast` (3, and 45 through 50).

That is 39 lines of the shared list. The cascade lines of each owned file go
with it. Several of the owned directives also need a stack switch, such as the
stackful directives of `big-interleaving-test.wast` and
`cancel-and-exclusive-lock.wast:196`. They pass under a provider. Without one, a
live run decides whether each one enters the no-provider overlay of [PDD022].
The design does not predict it.

These lines of owned files stay deferred, each with its current cause:

- `reentrance.wast:685`. It needs the rule of [PDD022] that a task that must not
  block runs the ready threads of its own instance.
- `reentrance.wast:813`, and `trap-if-block-and-sync.wast:340` and `:348`. They
  need a stack switch of a sync-typed thread, which [PDD022] states.
- `binary.wast` 1186 through 1275. The reference now requires the vestigial byte
  to be zero, and the parser still reads it as the `cancellable` immediate. The
  polyfill honors the immediate, so the eight lines stay in the `validation`
  category.
- The two WASI HTTP fixtures, which need a host for `wasi:http/types`.

## User Stories

A developer runs a Rust component whose HTTP handler has a timeout.

> The handler awaits an upstream request through a host `async` function. When
> its timer fires first, the guest's runtime calls `subtask.cancel` on the
> upstream subtask. The polyfill drops the host future in the next turn, and the
> guest reads `CANCELLED_BEFORE_RETURNED`. The borrow the guest lent to the
> request comes back, and the handler answers with a timeout.

A developer embeds a component that has a bug.

> One export traps on a divide by zero. The host's `Func::call` returns the
> trap. The developer calls another export of the same store, and that call
> fails with "cannot enter component instance". The developer knows that the
> store is lost and builds a new one. No other guest code ran after the trap, so
> the component's broken state was never read.

A developer ports a C library that uses pthreads and wit-bindgen.

> A worker thread waits with the cancellable form of `thread.yield` so that it
> can stop early. The same binary runs under Wasmtime and the polyfill. When the
> caller cancels, the yield returns 1 in both, the worker calls `task.cancel`,
> and the caller reads `CANCELLED_BEFORE_RETURNED`.

A developer passes an error from one component to another.

> A storage component fails a write and returns an `error-context` in a
> `result`. The caller component reads the debug message with
> `error-context.debug-message` and logs it. It then returns the same error
> context to the host, which gets `Val::ErrorContext` and passes it on to a
> third component. The record lives until the store drops.

## Test Cases

A trap poisons the store. `builtin-trap-poisons-instance.wast` passes lines 10
and 39 on both targets with the cannot-enter message. A repository test proves
each trigger on both targets:

- A guest trap.
- A built-in trap.
- A failed host function under a synchronous lower.
- A failed host `async` future.
- A mistyped host `Val`.
- A trap in a core `start` function.
- A trap in a destructor.

After each one, `Func::call` fails with the cannot-enter message. A second test
proves that an arity mismatch, a recursive driver, and a link error leave the
store usable.

A poisoned store refuses every guest entry. A repository test poisons a store
and then makes each entry on both targets: `Func::call`, `TypedFunc::call`,
`call_concurrent`, `Linker::instantiate`, a core module's instantiation, and
`Store::resource_drop` of a guest-defined resource. Each fails with the
cannot-enter message. `Store::resource_drop` of a host-defined resource
succeeds. A `run_concurrent` whose closure does only host work returns the
closure's value.

A poisoned store discards its work. A repository test starts a host `async`
function whose future sets a flag when it is dropped. The test also queues a
callback of a second task. A trap in a third task poisons the store, and the
flag is set at that moment. A later `run_concurrent` runs no callback. The
second task's guest code never runs again.

The first trap ends the driver that is polling. A repository test proves each
shape on both targets, and the driver returns the trap in each:

- A callback task that resolved in one turn and traps in a later turn.
- A thread that outlives its task's host call and traps.
- A callback export that fails after it resolved.
- A host `async` function that fails, called by a task that no host call owns.
- Two concurrent calls inside `run_concurrent`, where one traps.

In the last shape, `run_concurrent` returns the trap and the other call future
is dropped. In each shape, the next driver fails with the cannot-enter message.

`subtask.cancel` cancels a guest callee. With a provider, these pass on both
targets:

- `cancel-subtask.wast`, `cancel-delivery.wast`, and
  `cancel-and-exclusive-lock.wast`.
- `cancel-sync-and-waitable.wast`, `cancel-sibling-subtask.wast`, and
  `yield-when-cancelled.wast`.
- `trap-if-sync-and-waitable-set.wast`.
- The four owned directives of `big-interleaving-test.wast`.
- `reentrance.wast:548` and `:891`.
- `trap-if-block-and-sync.wast:352`, with the cannot-block message.

A repository test proves each resolution. A callee at the gate resolves to
`CANCELLED_BEFORE_STARTED`. A callee that calls `task.cancel` resolves to
`CANCELLED_BEFORE_RETURNED`. A callee that returns anyway resolves to
`RETURNED`. A second test proves that a stackful callee without a cancellable
built-in is never told. A synchronous cancel of that callee waits for its
return.

`subtask.cancel` cancels a host callee. `cancel-host.wast` passes whole on both
targets. A repository test proves three rules:

- The future is dropped in a turn and not inside the built-in.
- An asynchronous cancel returns `BLOCKED` first.
- A future that completed before the abort resolves to `RETURNED` with its
  result.

The cancellable immediate behaves as in Wasmtime 49. The corpus has no directive
for it. Repository tests prove each rule on both targets, with a provider:

- A pending request makes each cancellable built-in return the cancelled result
  at once.
- A blocked cancellable `waitable-set.wait` wakes with the task-cancelled event.
- A ready cancellable yield runs first and returns 1.
- The request does not wake a cancellable suspend.
- A built-in without the immediate never takes a request.

`task.cancel` and `subtask.cancel` trap with Wasmtime's messages. A repository
test proves each message of Error Model Growth that the corpus does not reach.
It runs on both targets. It includes the polyfill's own message for a second
cancel.

A cancelled subtask leaves nothing behind.
`cancel-starting-subtask-does-not-leak.wast` passes on both targets with the
harness's `wasmtime.set-max-table-capacity`. A repository test proves that a new
record past the cap fails with "resource table has no free keys". It also proves
that the default cap is 1,000,000.

Error contexts run. `error-context.wast` and
`error-context-trap-in-post-return.wast` pass whole on both targets. A
repository test sends an error context to another component in three ways: in a
parameter, in a list, and as the payload of a stream. The destination reads the
debug message and drops each handle. The record leaves the store when the last
handle drops. A second test lifts an error context to the host, as a `Val` and
as the typed `ErrorContext`. The guest then drops its handle, and the host
lowers its value into another component. The debug message reads the same there.

The lists record the result. The shared list holds none of the 39 lines this
design owns. The web list is unchanged. `tests all` runs the corpus in the four
states of [PDD022], and each state is green against its lists.

## References

- [PDD008], the Canonical ABI, whose string messages the error-context built-ins
  use.
- [PDD014], borrow lifetime tracking, whose scope-exit rule `task.cancel` keeps.
- [PDD016], the conformance suite.
- [PDD018], the concurrency runtime model, whose drivers and records this design
  completes.
- [PDD019], tasks and the callback export.
- [PDD020], subtasks and the async import.
- [PDD021], streams and futures.
- [PDD022], stack switching, stackful exports, and threads.
- [Explainer – invariants], the lockdown state of a component instance.
- [CanonicalABI – canon lift], which states that a trap tears down the store.
- [Concurrency], the Concurrency explainer, and its section on
  [cancellation][Concurrency – cancellation].
- [blast zones], the deferred feature for contained traps.
- [`definitions.py`], the executable reference for `Task`, `Subtask`,
  `canon_lift`, `canon_subtask_cancel`, `canon_task_cancel`, and the
  error-context built-ins.
- [spec #716], the change that removed the `cancellable` immediate.
- [Wasmtime], the reference implementation at `v49.0.0-rc.1`:
  - The poisoned store: its [trapped flag][Wasmtime trapped], [core
    trap][Wasmtime core trap], [call trap][Wasmtime call trap], and the three
    entry refusals ([call][Wasmtime call enter], [prepared
    call][Wasmtime prepared enter], [resource drop][Wasmtime drop enter]).
  - Its [event loop][Wasmtime event loop].
  - Cancellation: its [`subtask.cancel`][Wasmtime subtask cancel], [cancellable
    wait][Wasmtime cancellable wait], and [cancellable
    suspend][Wasmtime cancellable suspend].
  - Error contexts: its [built-ins][Wasmtime error context],
    [transfer][Wasmtime error-context transfer], and [host
    type][Wasmtime error-context any].
  - Its [resource table][Wasmtime resource table], its [wast
    runner][Wasmtime runner], and its [trap messages][Wasmtime traps].
- wit-bindgen's [C generator][wit-bindgen C], which emits the cancellable thread
  built-ins.
- The [Component Model test corpus] and the [Wasmtime component tests].

[PDD008]: ./PDD008%20Canonical%20ABI%20and%20Host%20Functions.md
[PDD014]: ./PDD014%20Borrow%20Lifetime%20Tracking.md
[PDD016]: ./PDD016%20Conformance%20Suite.md
[PDD018]: ./PDD018%20Concurrency%20Runtime%20Model.md
[PDD019]: ./PDD019%20Tasks%20and%20the%20Callback%20Async%20Export.md
[PDD020]: ./PDD020%20Subtasks%20and%20the%20Async%20Import.md
[PDD021]: ./PDD021%20Streams%20and%20Futures.md
[PDD022]: ./PDD022%20Stack%20Switching,%20Stackful%20Exports,%20and%20Threads.md
[Explainer – invariants]:
  https://github.com/WebAssembly/component-model/blob/main/design/mvp/Explainer.md#component-invariants
[CanonicalABI – canon lift]:
  https://github.com/WebAssembly/component-model/blob/main/design/mvp/CanonicalABI.md#canon-lift
[Concurrency]:
  https://github.com/WebAssembly/component-model/blob/main/design/mvp/Concurrency.md
[Concurrency – cancellation]:
  https://github.com/WebAssembly/component-model/blob/main/design/mvp/Concurrency.md#cancellation
[blast zones]:
  https://github.com/WebAssembly/component-model/blob/main/design/mvp/FutureFeatures.md#blast-zones
[`definitions.py`]:
  https://github.com/WebAssembly/component-model/blob/main/design/mvp/canonical-abi/definitions.py
[spec #716]: https://github.com/WebAssembly/component-model/pull/716
[Wasmtime]: https://github.com/bytecodealliance/wasmtime
[Wasmtime trapped]:
  https://github.com/bytecodealliance/wasmtime/blob/v49.0.0-rc.1/crates/wasmtime/src/runtime/component/store.rs#L285-L298
[Wasmtime core trap]:
  https://github.com/bytecodealliance/wasmtime/blob/v49.0.0-rc.1/crates/wasmtime/src/runtime/func.rs#L1476-L1479
[Wasmtime call trap]:
  https://github.com/bytecodealliance/wasmtime/blob/v49.0.0-rc.1/crates/wasmtime/src/runtime/component/func.rs#L389-L391
[Wasmtime call enter]:
  https://github.com/bytecodealliance/wasmtime/blob/v49.0.0-rc.1/crates/wasmtime/src/runtime/component/func.rs#L468-L470
[Wasmtime prepared enter]:
  https://github.com/bytecodealliance/wasmtime/blob/v49.0.0-rc.1/crates/wasmtime/src/runtime/component/concurrent.rs#L6278-L6280
[Wasmtime drop enter]:
  https://github.com/bytecodealliance/wasmtime/blob/v49.0.0-rc.1/crates/wasmtime/src/runtime/component/resources/any.rs#L204-L206
[Wasmtime event loop]:
  https://github.com/bytecodealliance/wasmtime/blob/v49.0.0-rc.1/crates/wasmtime/src/runtime/component/concurrent.rs#L1325-L1340
[Wasmtime subtask cancel]:
  https://github.com/bytecodealliance/wasmtime/blob/v49.0.0-rc.1/crates/wasmtime/src/runtime/component/concurrent.rs#L4161-L4345
[Wasmtime cancellable wait]:
  https://github.com/bytecodealliance/wasmtime/blob/v49.0.0-rc.1/crates/wasmtime/src/runtime/component/concurrent.rs#L4085-L4103
[Wasmtime cancellable suspend]:
  https://github.com/bytecodealliance/wasmtime/blob/v49.0.0-rc.1/crates/wasmtime/src/runtime/component/concurrent.rs#L4005-L4065
[Wasmtime error context]:
  https://github.com/bytecodealliance/wasmtime/blob/v49.0.0-rc.1/crates/wasmtime/src/runtime/component/concurrent/futures_and_streams.rs#L4245-L4330
[Wasmtime error-context transfer]:
  https://github.com/bytecodealliance/wasmtime/blob/v49.0.0-rc.1/crates/wasmtime/src/runtime/component/concurrent/futures_and_streams.rs#L4558-L4589
[Wasmtime error-context any]:
  https://github.com/bytecodealliance/wasmtime/blob/v49.0.0-rc.1/crates/wasmtime/src/runtime/component/values.rs#L1260-L1268
[Wasmtime resource table]:
  https://github.com/bytecodealliance/wasmtime/blob/v49.0.0-rc.1/crates/wasmtime/src/runtime/component/resource_table.rs#L20-L75
[Wasmtime runner]:
  https://github.com/bytecodealliance/wasmtime/blob/v49.0.0-rc.1/crates/wast/src/wast.rs#L258-L270
[Wasmtime traps]:
  https://github.com/bytecodealliance/wasmtime/blob/v49.0.0-rc.1/crates/environ/src/trap_encoding.rs
[wit-bindgen C]:
  https://github.com/bytecodealliance/wit-bindgen/blob/2f795ab/crates/c/src/lib.rs#L750-L780
[Component Model test corpus]:
  https://github.com/WebAssembly/component-model/tree/main/test/async
[Wasmtime component tests]:
  https://github.com/bytecodealliance/wasmtime/tree/main/tests/misc_testsuite/component-model/async
