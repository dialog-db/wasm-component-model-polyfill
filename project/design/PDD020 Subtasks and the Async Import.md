# Subtasks and the Async Import

[PDD018] designed the runtime that every concurrency feature shares. [PDD019]
built the first feature on it, the callback export called from the host. This
document designs the second feature: the asynchronous lower. An asynchronous
lower is a `canon lower` with the `async` option. Through it a guest calls a
host function or another component's export and gets control back before the
callee returns. Every such call creates a subtask, the record of one call out
through an import. The feature brings the subtask record and its events,
`subtask.drop`, the prepare-and-start call protocol of the fused adapters, the
host `async` function on `LinkerInstance`, the concurrent call entry on `Func`
and `TypedFunc`, and the reentrance rules. It also revises the fallback of the
suspend seam of [PDD018], so that a task that is allowed to block can wait for
guest work on a target with no stack switch.

The design follows the [Concurrency explainer][Concurrency] and the Python
reference in [`definitions.py`] at the commit the conformance corpus of [PDD016]
is vendored from. Where the reference leaves a choice to the host, the design
makes the choice [Wasmtime] makes at `v49.0.0-rc.1`. That tag is the first to
carry Wasmtime's realignment with the current reference. It allows reentrance
everywhere, blocks a sync-typed task lazily, and runs the callee thread of a
call next. Its trap messages equal those of Wasmtime 48. The fused adapters of
Wasmtime 48 trap on a call between a parent and a child instance, which the
reference forbids, so this design requires the fused adapter compiler of
Wasmtime 49 or later. Where the design departs from the reference or from
Wasmtime, it states the reason.

Two terms recur. An async-typed function is a function whose type carries the
`async` effect. A sync-typed function is one whose type does not. The lift and
the lower are separate axes: an async-typed export can be lifted synchronously,
and a sync-typed import can be lowered asynchronously.

## Goals

- The translator accepts `canon lower async`, an async-typed import, and the
  prepare-and-start trampolines of the fused adapters.
- A guest call into a host `async` function or into another component's export
  is a subtask. The subtask enters the caller's handle table when the call does
  not resolve at once, and the guest waits on it as on any waitable.
- A call between components with an asynchronous lower or an asynchronous lift
  runs through the prepare-and-start protocol. When the entry gate is open, the
  callee starts before the lower returns, and the status word is the
  reference's.
- `subtask.drop` removes the entry with the trap of the reference.
- The subtask event reaches the caller through the waitable sets of [PDD019].
  `waitable.join` finds its first waitable kind.
- A task that is allowed to block waits for guest work through the nested turn
  of [PDD018], with five revised rules. An idle store is a deadlock. A pending
  host task needs a stack switch. A wait the store never serves runs out of the
  seam's one budget and fails with the stack-switch cause.
- A host registers a host `async` function through two entries named as Wasmtime
  names them. The future reaches the store only through the accessor, which is a
  token.
- The resolver enforces Wasmtime's rule on the registration kind: an async-typed
  import needs a concurrent registration, and a sync-typed import refuses one.
- Two host calls into one instance overlap through `call_concurrent`.
- No call traps for reentrance. The entry gate is the only serialization.
- Every trap the feature adds is a structured error cause with Wasmtime's
  message, so the conformance corpora match it by substring.
- The behavior is the same on both targets.

## Non-goals

- `subtask.cancel`, `task.cancel`, and cancellation delivery. Cancellation is
  one mechanism with two ends, and one design owns both. `subtask.cancel` stays
  refused at translation. The cancel-requested flag of the subtask record stays
  unused. The two cancelled states are reached here only through the failure
  path [PDD018] has, where a host task's body fails.
- A callee whose core function blocks before it returns and whose block only its
  caller can release. That shape needs a stack switch. The polyfill fails it
  with the stack-switch cause, and the corpus files that need it stay deferred.
- The stackful form of `canon lift async` and the thread built-ins other than
  `thread.yield`. They stay refused at translation.
- Streams, futures, and error contexts, as built-ins and as value types. The
  type projection still refuses them in a function type.
- The rules that decide which trap poisons an instance. `CannotEnterComponent`
  is Wasmtime's trap for a host entry into a poisoned instance. This design
  raises it nowhere and reserves it for those rules.
- A provider for the suspend seam.
- Host-side cancellation of a task that `call_concurrent` started. Dropping the
  store is the only way to end one, which is Wasmtime's rule at this version.

## Blocking Without a Stack Switch

[PDD018] serves a blocking built-in with a nested turn when the target has no
suspend provider. A nested turn runs from inside the guest call, runs ready
guest work, polls the host tasks, and returns when the condition holds or
nothing can progress. This document revises its rules. The revision applies to
every blocking built-in of [PDD019] and of this design, and to `thread.yield` as
[PDD019] states it.

Five rules hold:

- Yielded items run inside a nested turn. The driver turn keeps the yield rule
  of [PDD018] and returns control to the host executor before a yielded item
  runs. A nested turn has no executor to return to. It runs the yielded item
  once no other item is ready. The reference asks only that other ready threads
  get their chance, and they do. A yielded item that runs and gives way again is
  ready again, and the next nested turn runs it again. No rule holds an item
  back once it has had a chance, and the rule below is what ends a wait on an
  item that never converges.
- An idle store is a deadlock, and a pending host task needs a stack switch. The
  store is idle when nothing is ready and no host task is pending, the blocked
  call's own future included. A stack switch does not help there, so the
  built-in fails with the deadlock cause. When a host task is pending, only a
  real suspension can wait for the executor, so the built-in fails with the
  stack-switch cause.
- A sync-typed call in progress turns both failures into the cannot-block cause.
  The reference runs the ready threads of a sync-typed task's own instance while
  the task blocks, and traps when none remains. A sync-typed task's block
  therefore runs only items of its own instance. Items of other instances stay
  queued for the driver. An async-typed task's block runs any ready item,
  because the reference returns control to the caller there and any thread can
  run. When the nested turn cannot progress and any instance in the store has a
  sync-typed call in progress, the cause is the cannot-block cause, as Wasmtime
  reports it on idle.
- The seam keeps one budget, and past it the call fails with the stack-switch
  cause. This is the one departure of this design from the reference, and the
  section below states it.
- Nested turns nest. An item that a nested turn runs can block and open another
  nested turn on the real stack. The guest's own call nesting bounds the depth.

```text
fn block(store, condition) -> Result:
    // a nested turn runs ready items, then yielded items, then polls the
    // host tasks, and reports Progress, Idle, or Waiting
    filter = own_instance if current_task().must_not_block() else any_instance
    past_budget = false
    loop:
        if condition(): return Ok
        outcome = store.nested_turn(active_waker, filter)
        past_budget = seam.note_turn(store)
        match outcome:
            Progress if not past_budget: continue
            _:                           break
    if condition():   return Ok
    if past_budget:   return Err(StackSwitchNeeded)
    if any_sync_typed_call_in_progress(): return Err(CannotBlock)
    if host_tasks_pending():             return Err(StackSwitchNeeded)
    return Err(Deadlock)

fn give_way(store) -> Result:                 // what thread.yield asks for
    filter = own_instance if current_task().must_not_block() else any_instance
    store.nested_turn(active_waker, filter)
    if seam.note_turn(store): return Err(StackSwitchNeeded)
    return Ok
```

A nested turn runs only when the suspend seam has no provider. A provider
suspends the thread and ends the turn instead, as [PDD018] states, and the
driver's turn then serves the same work under the same rules. A target with a
provider never consults the budget.

### The One Budget

Two shapes make a nested turn run for ever, and they are one shape. A callee
that spin-waits in its event loop until its caller unblocks it gives way, is
queued again, runs again and gives way again. When the block that runs it is
that caller, every turn runs it and no turn gets anywhere. A callee whose core
function calls `thread.yield` in a loop against a store that holds nothing asks
the seam over and over for what it was refused the time before. In both, the one
thread that can release the waiting thread is a guest frame on the real stack
below it. The reference reaches that frame by switching stacks. The polyfill
cannot, so an unbounded wait here never ends.

The seam therefore keeps one budget, and it is the polyfill's own: the reference
bounds neither the yielded item nor the number of times a thread gives way. The
seam counts the nested turns in a row in which the store did nothing of its own.
A turn does nothing of its own when it runs no item, or nothing but a resumption
after a yield, against a store that holds no host future that can still resolve.
A turn that runs any other item, and a turn taken while such a future is
pending, start the count over. The count is one count for the store, measured
from the last turn the seam noted, so a thread that gives way in one call frame
after another builds one run, and whatever the store runs in between ends it.

Once the run passes the budget, the seam gives up and the call the waiting
thread is inside fails with the stack-switch cause. The cause names the stack
switch and not the deadlock or the cannot-block cause, because the store is not
idle and no rule of the caller's is broken: the work that can release the thread
exists and sits where only a stack switch reaches it. That is what a guest
observes. A callee that spin-waits for its caller sees its call trap with the
stack-switch message, and so does the call under which a thread gives way for
ever.

The budget is a budget and not a proof. Nothing short of running the guest to
its end tells a loop that gives way this many times and then returns from one
that never returns, so the number is drawn generously: a converging loop of any
ordinary length finishes well inside it, and a spinning one reaches its failure
in a bounded number of turns. Two consequences follow, and the design accepts
both. A loop that converges after more times than the budget is cut short. A
guest loop that gives way for ever with no caller below it, which no stack
switch releases either, reaches the same failure as one that waits for a caller,
because the seam reads the store and not the stack.

`thread.yield` itself has no rule that fails. It gives way through the seam and
returns zero whenever it returns, as [PDD019] states. What fails past the budget
is the call the giving thread is inside.

## The Async-Typed Import and Its Lowered Signature

The type projection of [PDD019] accepts an async-typed export. This design
accepts an async-typed import too. `FunctionType.async_` is true on it, and the
resolver's link-time check reads the flag as the link rule below states.

The core signature of a lowered import follows the reference's flattening for
its lower. A synchronous lower keeps the signature of [PDD008]. An asynchronous
lower has at most four flat parameters. When the flattened parameters exceed
four, it takes one pointer instead. When the type has a result, it always
returns the result through a pointer parameter. It returns one `i32`, the status
word. Validation requires the `memory` option on an asynchronous lower. The
`LowerImport` trampoline derives the signature from the options of the lower.

The status word is the one [PDD018] defines. Its low four bits are the subtask
state and its high bits are the subtask's index in the caller's handle table.
The state `RETURNED` (2) comes with no index. `STARTING` (0) and `STARTED` (1)
come with the index of the entry the caller waits on.

## The Subtask Record and Its Events

A subtask is the record of one call out through an import, as [PDD018] defines
it. This design fills in the parts that the asynchronous lower observes.

- The record starts in `STARTING`. It moves to `STARTED` when the callee has
  read its parameters, and to `RETURNED` when the callee has produced its
  result. The two cancelled states are resolved states that only a failure
  reaches here.
- The record enters the caller's handle table only when the lower returns with a
  state other than `RETURNED`. Indices come from the caller instance's allocator
  in call order, as [PDD018] states for every handle. A call that resolves
  before the lower returns leaves no entry.
- A subtask holds one pending event slot. The scheduler fills it when the
  subtask resolves. It also fills it when the subtask starts, if the caller
  already holds a handle, which is the case of a callee the gate held and later
  let through. Delivery reads the subtask's state at that moment, so a `STARTED`
  that was never delivered reads as `RETURNED`. The event is the triple of
  [PDD018]: the subtask code (1), the handle index, and the state.
- Delivery of a resolved state marks the resolution as delivered and releases
  every handle the caller lent for the call. A synchronous lower delivers the
  resolution as it returns.
- `subtask.drop` takes a handle index. It traps when the index does not name a
  subtask, when the resolution was not delivered, and when the instance's
  may-leave flag is clear. Otherwise it removes the entry. A guest callee's task
  record leaves the store once its implicit thread has exited and its entry is
  gone. A host task's record leaves the store with its entry.

`waitable-set.wait` and `waitable-set.poll` of [PDD019] deliver the subtask
event, and `waitable.join` moves a subtask between sets. None of them changes.

## Calls Between Components

With the fused adapter compiler of Wasmtime 49, a call between two components
takes one of two paths. When the lower and the lift are both synchronous, the
adapter calls the enter and exit intrinsics of [PDD018] and nothing else. When
either is asynchronous, the adapter calls `PrepareCall` and then `SyncStartCall`
or `AsyncStartCall`. The polyfill serves the three intrinsics as trampolines.

Two generated functions accompany every prepared call. The start function takes
the caller's flat arguments, lifts them in the caller, and lowers them into the
callee. The return function takes the arguments of the callee's `task.return`,
or a sync export's flat results, and lowers them into the caller. The adapter
code itself handles realloc, the may-leave flag, and fresh context slots around
each realloc call. The polyfill only calls the two functions at the right
moments and serves the intrinsics they use.

```text
fn prepare_call(caller_instance, callee_instance, callee_function, start, return_,
                caller_kind, flat_args):
    task = store.create_task(callee_function, callee_lift_options, callee_instance)
    task.caller = current_thread()
    subtask = store.create_subtask(scope = task)        // state: starting
    subtask.start = (start, flat_args)
    subtask.return_ = return_
    subtask.caller_kind = caller_kind                   // sync, or async with or without a result

fn start_call(lower_kind):
    item = TaskStart(task):
        flat = call(subtask.start)                      // lifts and lowers the arguments
        subtask.state = started
        if subtask.handle is set: subtask.record_event()
        result = call_and_trap(callee.core, flat)
        match callee.lift:
            sync:     call(subtask.return_, result)     // lowers into the caller
                      subtask.resolve(returned)
                      run post-return
                      task ends
            callback: handle_status_word(task, result)  // as the callback export states
    store.scheduler.switch_slot = item
    store.enter_implicit_thread(task)                   // a held task clears the slot
    store.run_switch_slot()                             // one item, when the gate let it pass
    match lower_kind:
        async:
            if subtask.resolved(): return RETURNED
            subtask.handle = caller.handles.insert(subtask)
            return subtask.state | (subtask.handle << 4)
        sync:
            block(store, condition = subtask.resolved())
            subtask.deliver_resolution()
            return subtask.flat_results

fn on_task_return(task, flat_results):                  // task.return in a callback export
    if task.subtask is set:
        call(task.subtask.return_, flat_results)        // lowers into the caller
        task.subtask.resolve(returned)                  // records the subtask event
```

The rules of the protocol:

- Start runs the callee now. The callee's start item goes in the switch slot of
  [PDD018] and runs from inside the trampoline. That is the nested turn
  restricted to the switch slot. The reference resumes the callee's thread
  before the lower returns, and the corpus reads `STARTED` right after a call
  whose gate was open. A suspend provider suspends the caller instead and lets
  the driver run the item.
- The gate holds the item as [PDD019] states. A callback export and a
  synchronous export of an async-typed function need the exclusive thread of
  their instance. A held item leaves the subtask in `STARTING`, and the item
  stays queued in arrival order. A synchronous export of a sync-typed function
  ignores the gate.
- When the callee's `task.return` runs, the return function runs at that moment.
  This is the reference's `on_resolve`. The result crosses into the caller's
  memory through the caller's boundary context, before the callee's callback
  continues. For a sync export, the results cross when the core function returns
  and post-return runs afterwards.
- A callee that parks leaves its caller free. A task parks when it waits in its
  event loop with no frame on the stack. A callback export that returns the
  yield or wait word parks, so the lower returns `STARTED` to the caller. A
  callee whose core function blocks inside a built-in keeps its frame on the
  stack below the trampoline. The nested turn serves that block with work that
  is ready or becomes ready without the caller. When only the caller can release
  it, the call fails with the stack-switch cause.
- A synchronous lower of an async-typed callee blocks the caller until the
  callee resolves, under the rules of the blocking section. A sync-typed caller
  that has to wait fails with the cannot-block cause, and only after the callee
  did not resolve at once. That is the lazy rule of the reference and of
  Wasmtime 49.
- A trap in the callee, or an exception the callee does not catch, unwinds
  through the start item to the trampoline and fails the caller's call. The
  message is the one the synchronous baseline gives the same trap.

The four combinations, and what the caller observes:

| Lower        | Lift        | Intrinsics              | The caller sees                                |
| ------------ | ----------- | ----------------------- | ---------------------------------------------- |
| synchronous  | synchronous | enter and exit          | the flat results                               |
| synchronous  | callback    | prepare and sync start  | the flat results, after the callee resolved    |
| asynchronous | synchronous | prepare and async start | `RETURNED`, or `STARTING` while the gate holds |
| asynchronous | callback    | prepare and async start | `RETURNED`, `STARTED`, or `STARTING`           |

An asynchronous lower of a sync export whose task the gate holds returns
`STARTING`. When the gate opens, the driver runs the start item, the results
cross, and the subtask event carries `RETURNED`.

## The Host `async` Function

A host `async` function is a host function whose call produces a future the
store owns and polls. [PDD018] fixed the contract of that future and of its host
task. This design adds the registration, the accessor's shape, and the link
rule.

### Registration

`LinkerInstance` gains two entries, named as Wasmtime names them:

```rust
impl<T: 'static> LinkerInstance<'_, T> {
    pub fn func_wrap_concurrent<Params, Ret, F, Fut>(&mut self, name: impl Into<String>, func: F)
    where
        Params: ComponentParameters,
        Ret: ComponentResult,
                F: Fn(&Accessor<T>, Params) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<Ret>> + 'static;       // and Send natively

    pub fn func_new_concurrent<F, Fut>(&mut self, name: impl Into<String>, ty: FunctionType, func: F)
    where
        F: Fn(&Accessor<T>, Vec<Val>) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<Vec<Val>>> + 'static;  // and Send natively
}
```

The typed entry derives the function type from the closure's types, as
`func_wrap` does. The untyped entry takes the type at registration, as
`func_new` does, rather than passing it to the closure as Wasmtime does. The
polyfill checks the type at link time, and the closure gains nothing from it.
The untyped entry takes owned values in and owned values out, where Wasmtime
lends slices. The future outlives the trampoline's frame, since it lives in the
store as a host task, so nothing it borrows from the call survives. When the
future completes, the result vector must hold one value when the type declares a
result and none otherwise. A mismatch fails the call as the untyped synchronous
path fails it.

The future's bound is the host future bound of [PDD018]: `Send` and `'static`
natively, `'static` alone in the browser. A JavaScript promise wrapped as a
future satisfies the browser bound.

### The Accessor Is a Token

The accessor of [PDD018] reaches the store's host data from a future that does
not borrow the store. This design revises its shape. `Accessor<T>` carries no
store and no lifetime. It holds the store's identity and nothing else. The store
owns one slot per thread. Around each poll of a host task's body, and around
each poll of the `run_concurrent` closure, the store places a pointer to its own
context in the slot and takes it out again before the poll returns. Outside a
poll the slot is empty. This is Wasmtime's design: its accessor holds a store
token, and `with` reads the store from thread-local storage set around each
poll.

```text
fn with(accessor, body) -> Result<R>:
    store = slot.take()                       // empty outside a poll, and inside `with`
    fail_if(store is none, StoreNotInPoll)
    fail_if(store.id != accessor.store_id, StoreNotInPoll)
    result = body(store)
    slot.put(store)
    return Ok(result)
```

Every rule of [PDD018] stays. A body reaches the store only inside `with`. A
value taken from the host data must be cloned out. `with` inside `with` finds
the slot empty and fails with the recursive-driver cause, as today. A `with`
outside any poll, or with an accessor of another store, fails with the
store-not-in-poll cause, where Wasmtime panics. The visible change is the
lifetime leaving the public type, so `Store::run_concurrent` hands its closure
`&Accessor<T>`, and a host task's future holds the same reference across its
awaits. One slot suffices because a poll holds the store by `&mut`, so at most
one store is inside a poll on a thread. In the browser the slot is one static
cell, because the page has one thread, and the declaration is the same.

Everything `HostCall` offers a synchronous host function reaches a host `async`
function through `with` on the store context: the host data, and `resource_new`
for a resource type the calling instance knows.

### The Link Rule

Wasmtime 49 refuses at link time an async-typed import satisfied by `func_new`
or `func_wrap`, and a sync-typed import satisfied by `func_new_concurrent` or
`func_wrap_concurrent`. The resolver of [PDD007] enforces both, and each failure
is a link error with Wasmtime's message. A synchronous host function serves an
async-typed import correctly, since it resolves at once, so the rule is
Wasmtime's choice rather than the reference's. The polyfill follows it so that a
host's registrations move between the two unchanged.

### The Call

Through an asynchronous lower, the call is the one [PDD018] designs. The
trampoline polls the future once. A ready future lowers its result and the guest
sees `RETURNED`. A pending future joins the store's host tasks, the subtask
enters the caller's handle table in `STARTED`, and the guest sees that state
with the index. A later turn lowers the result through the boundary context of
the subtask and fills the subtask event.

Through a synchronous lower, the call blocks the guest thread where it stands.
[PDD018] let the call succeed only when the first poll resolved the future or a
provider filled the seam. This design revises that rule. The block runs under
the blocking section, and the blocked call's own future is polled at every
check. A future that completes after a few polls, as a future that yields once
does, resolves inside the nested turn and the call returns its result. A future
that stays pending leaves the store waiting, so the call fails with the
stack-switch cause, or with the cannot-block cause when the caller is a
sync-typed task that has not returned.

Handles the guest lends for the call go on the subtask's lender list and come
back when the resolution is delivered. A borrow lent to a host task therefore
stays lent until the guest takes delivery of the subtask event, and a guest that
drops the owning handle before then traps as [PDD014] states.

## The Concurrent Call Entry

`Func` and `TypedFunc` gain one entry each, named as Wasmtime names them and
shaped as the polyfill's own calls are:

```rust
impl Func {
    pub async fn call_concurrent<T: 'static>(&self, accessor: &Accessor<T>, args: &[Val]) -> Result<Box<[Val]>>;
}

impl<P: ComponentParameters, R: ComponentResult> TypedFunc<P, R> {
    pub async fn call_concurrent<T: 'static>(&self, accessor: &Accessor<T>, params: P) -> Result<R>;
}
```

The entry reaches the store through `with`, creates the export's task, and
queues its start item behind the entry gate, exactly as `Func::call` does. It
returns a future that resolves when the task returns, and lifts the result.
Post-return runs after the result is lifted, as it does for `Func::call`. The
future is spawn-like, as Wasmtime documents its own. Dropping it cancels
nothing. The task progresses only while a driver runs turns, in practice while
the future is awaited inside the `run_concurrent` closure. A `Func::call` is a
driver and cannot be entered from the closure, because it takes the store by
`&mut` and the closure holds only the accessor.

Two overlapping calls observe the entry gate and the scheduling order of
[PDD018]:

- Two calls into one callback export: the second waits at the gate while the
  first runs core code, starts when the first returns a status word, and the two
  then interleave by events.
- A call into a synchronous export while a callback task of the same instance
  waits in its event loop runs at once, because the exclusive thread is released
  between events.
- A second call into a synchronous export while a first synchronous task runs is
  queued. It starts when the first returns, or inside the first's nested turn
  when the first blocks and the two share an instance.

When the store goes idle with the task unresolved, the `run_concurrent` entry
returns pending, as [PDD018] states, and the call's future never resolves. That
is Wasmtime's behavior. The call's future does not fail on idle, because the
closure can still unblock the task with another call, and a host bounds the
whole entry with a timeout.

## Reentrance

The reference at the corpus commit and Wasmtime 49 agree, and both differ from
Wasmtime 48 on this point:

- No call traps for reentrance. The adapters of Wasmtime 49 emit no
  `CannotEnterComponent` for a call between a parent and a child or into the
  caller's own instance. A sync-typed callee can be entered at any depth: from a
  child, a parent, a sibling, a destructor, or the host. The synchronous
  baseline serves each as a nested call on the real stack.
- The entry gate is the only serialization. An async-typed callee lifted
  synchronously or with a callback needs the exclusive thread of its instance,
  so a reentrant call into it waits at the gate while the holder runs core code.
  A callback holder releases between events, and the call proceeds. A
  synchronous holder releases on return, so a cycle through it goes idle and
  fails with the deadlock cause.
- The host can always enter. Wasmtime 49 refuses a host entry only into a store
  a trap poisoned, and the poisoning rules are out of scope. From the host,
  reentrance while an instance is on the stack is reachable only through
  `call_concurrent`, from a host task's body or from the `run_concurrent`
  closure, and the gate treats it like any other call.
- The may-leave flag is unchanged. A lowered import called from a realloc or a
  post-return fails with the cannot-leave cause of [PDD019].

## Translation

The translator accepts what this design builds and refuses the rest with
`Error::Unsupported`, the rule [PDD006] states:

- Accepted: `canon lower async`, an async-typed import, the `PrepareCall`,
  `SyncStartCall`, and `AsyncStartCall` trampolines, `subtask.drop`, and a
  `funcref` parameter in an intrinsic signature, which the prepare and start
  intrinsics carry.
- Refused: `subtask.cancel`, `task.cancel`, `canon lift async` without a
  callback, the stream, future, and error-context built-ins, every thread
  built-in other than `thread.yield`, and the table initializer of
  `thread.new-indirect`.

The adapters of Wasmtime 49 import no may-block global. The may-not-suspend flag
of the instance record of [PDD018] carries that state. The enter intrinsic sets
it for a synchronous call between components, a host call into a sync-typed
export sets it for the length of the call, and each clears it on exit.

## Error Model Growth

- `LinkError` gains two causes. An async-typed import satisfied by a synchronous
  registration, and a sync-typed import satisfied by a concurrent one. Each
  carries Wasmtime's message for that mismatch.
- `SchedulerCause` gains one cause, store not in poll: an accessor was used
  while no poll of its store was running, or with an accessor of another store.
  Wasmtime panics there. The polyfill returns the error, because `with` already
  returns a result.
- The nested turn's failures are the existing causes under the revised rules:
  deadlock on idle, stack switch needed on a pending host task, cannot block
  when a sync-typed call is in progress, and stack switch needed once the seam's
  budget runs out.
- `subtask.drop` on an undelivered resolution is the subtask-not-resolved cause
  [PDD018] already defines, with Wasmtime's message.

No trap cause is added. `CannotEnterComponent` is raised nowhere.

## Target Differences

Nothing in this document differs between the native target and the browser
beyond the four differences [PDD018] states. The accessor's slot is a
thread-local natively and one static cell in the browser, under one declaration.
The host future bound is the per-target bound of [PDD018].

## The Corpus

The harness registers the five asynchronous items of Wasmtime's component
spectest through `func_wrap_concurrent`, and each behaves as Wasmtime's wast
runner makes it behave. `host-echo-u32` resolves at once with its argument.
`never-return` and `[method]resource1.never-return` stay pending. `echo-slowly`
and `return-two-slowly` are pending once and resolve at the next poll, which is
what a single yield on Wasmtime's executor does.

The files and directives this design owns, in the Component Model corpus:

- `cross-abi-calls.wast`, whole. Twenty-four directives pair synchronous and
  asynchronous lowers with synchronous and callback lifts over the flat and heap
  cases of the parameters and the result.
- `deadlock.wast`. A synchronous export of an async-typed function starts a
  callback export that waits on an empty set and then waits on the subtask. The
  nested turn goes idle and the call fails with the deadlock message.
- `dont-block-start.wast`, its second directive. A start function calls a
  callback export through a synchronous lower, the callee waits, and
  instantiation fails with the cannot-block message.
- `drop-subtask.wast`, both directives. A subtask dropped after its resolution
  was delivered succeeds, and one dropped before that traps.
- `drop-waitable-set.wast`. A callback export waits on a set, and a second
  callee drops that set and traps.
- `reentrance.wast`, eight of its twelve directives: the five synchronous
  reentrance cases, which the adapters of Wasmtime 49 no longer trap, the
  sync-typed reentry while the exclusive thread is held, the callback cycle that
  waits at the gate and completes, and the synchronous cycle that fails with the
  deadlock message.

In the Wasmtime corpus:

- `backpressure-deadlock.wast`. A subtask held at the gate by backpressure reads
  `STARTING`, and a wait on it fails with the deadlock message.
- `callback-yield-then-exit.wast`. A synchronous lower returns at the callee's
  `task.return`, and the callee's late exit runs in the next driver's turn.
- `context-in-compositions.wast`, its last four directives, which permute the
  four combinations of lower and lift.
- `drop-host.wast`. A host subtask whose future completed but whose event was
  not delivered cannot be dropped.
- `exceptions.wast`, whole. An exception thrown in the callee of each of the
  three asynchronous combinations, in its first phase and in its callback or
  after its yield, reaches the host as the trap the synchronous baseline gives
  it.
- `fused.wast`, whole. The three asynchronous combinations with a callee that
  resolves at once.
- `lower.wast`. A component that lowers `host-echo-u32` asynchronously
  instantiates.
- `many-params-with-retptr.wast`. A synchronous lower of a callback export with
  the maximum flat parameters and a return pointer.
- `reentrance.wast`. A callback export calls a child's callback export, which
  calls back into the root through a table and waits at the root's gate, and the
  host reads the root's result.
- `subtask-wait.wast`. A wait on a subtask runs the callee's yielded callback
  inside the nested turn, and the callback's trap reaches the host.
- `task-builtins.wast`, its `subtask.drop` component and its four directives
  that permute the combinations across two components.
- `wait-forever.wast` and `wait-forever2.wast`. A callee that waits forever,
  through a synchronous and through an asynchronous lower, fails with the
  deadlock message.

The files that stay deferred, with the reason:

- A stack switch: `async-calls-sync.wast`. Its synchronous lowers reach a
  callback export that yields until the outer caller unblocks it, so only the
  caller can release the callee.
- The stackful lift: `stackful.wast`, `drop-waitable-set-stackful.wast`,
  `reenter-during-yield.wast`, `sync-barges-in.wast`, and five directives of
  `task-return-traps.wast`.
- Thread built-ins: the `during-sync-call-*.wast` group,
  `during-sync-scheduling-candidates.wast`, `self-switch-traps.wast`,
  `switch-to-ready-callback.wast`, `trap-if-block-and-sync.wast`,
  `trap-if-sync-and-waitable-set.wast`, `join-during-sync-read.wast`,
  `task-deletion.wast`, the second directive of `task-return-traps.wast`, and
  the two thread directives of the Component Model `reentrance.wast`.
- Streams and futures: `empty-wait.wast`, `wait-during-callback.wast`,
  `drop-cross-task-borrow.wast`, `passing-resources.wast`,
  `cross-task-future.wast`, `trap-if-done.wast`,
  `trap-if-transfer-in-waitable-set.wast`, `sync-and-async-waitable.wast`,
  `waitable-set-stale-entry.wast`, `big-interleaving-test.wast`,
  `async-builtins.wast`, the stream and future case of `task-builtins.wast`, and
  every file named for a stream or a future.
- Cancellation: every file named for it, the two `subtask.cancel` directives of
  the Component Model `reentrance.wast`, and the `subtask.cancel` component of
  `task-builtins.wast`.
- Error contexts: `error-context.wast`.
- The trap rules: `builtin-trap-poisons-instance.wast`.

## User Stories

A developer registers a host function that fetches over the network, in a
browser page.

> The developer calls `func_wrap_concurrent` with a closure that returns an
> `async` block. The block awaits the fetch promise, then reaches the host data
> through `accessor.with` and stores the response. The guest called the function
> through an asynchronous lower, saw `STARTED`, and waited on the subtask. When
> the response lands, a turn lowers it and delivers the event.

A developer runs two calls into one instance at the same time.

> Inside `run_concurrent`, the developer awaits two `call_concurrent` futures
> together. Both tasks pass the gate in order, interleave by events, and the
> closure returns both results.

A component author composes two components.

> A callback export calls the sibling's export through an asynchronous lower and
> reads `STARTED`. It joins the subtask to its set and returns the wait word.
> When the sibling calls `task.return`, the result crosses into the caller's
> memory and the callback receives the subtask event.

A guest author reads a status word.

> The author sees `STARTING` and knows the callee has not read the parameters
> yet, so the argument buffer must stay valid. On `STARTED` the author frees the
> argument buffer. On `RETURNED` the author reads the result buffer.

A contributor adds a stack switch to one target.

> The contributor fills the suspend capability. Every block this design serves
> with a nested turn suspends the thread instead, and the driver's turn runs the
> same work under the same five rules, and the budget of the fifth is never
> consulted.

A reviewer reads the conformance summary after the feature lands.

> The reviewer sees the two `async` rows, the lines this design removed, the
> same counts on both targets, and the five asynchronous spectest items in the
> harness.

## Test Cases

The type projection accepts an async-typed import. A host reads `async_` as true
on an imported function's type. `lower.wast` instantiates. A repository test
proves the flag.

The link rule matches Wasmtime's. An async-typed import satisfied by `func_wrap`
fails to link with Wasmtime's message, and a sync-typed import satisfied by
`func_wrap_concurrent` fails with the other. Both typed and untyped entries
register and link for an async-typed import. Repository tests prove all four
cases.

An asynchronous lower of a host `async` function returns the right word. A
future ready on its first poll gives `RETURNED` and no entry. A pending future
gives `STARTED` with an index, and a later turn lowers the result and delivers
the subtask event with that index and `RETURNED`. A borrow lent for the call is
released only when the event is delivered. Repository tests prove each case, on
a hand-written component, in both registration forms.

A synchronous lower of a host `async` function resolves inside nested turns. A
future that is pending once and then ready returns its result. A future that
stays pending fails with the stack-switch cause from an async-typed caller and
with the cannot-block cause from a sync-typed caller. Repository tests prove the
three cases.

The four combinations cross correctly. `fused.wast` and `cross-abi-calls.wast`
pass whole. The four async permutations of `task-builtins.wast` and of
`context-in-compositions.wast` pass. `many-params-with-retptr.wast` passes.

The gate holds a callee. `backpressure-deadlock.wast` reads `STARTING` under
backpressure. The Wasmtime `reentrance.wast` holds a callback callee at the
root's gate while the root's own task runs and completes when it exits. The
seventh case of the Component Model `reentrance.wast` completes the callback
cycle. A repository test proves that a held sync export runs when the gate opens
and delivers `RETURNED`.

Subtask events and drops behave as the reference states. `drop-subtask.wast`,
`drop-host.wast`, and `subtask-wait.wast` pass, and the `subtask.drop` component
of `task-builtins.wast` instantiates. A repository test proves that a `STARTED`
never delivered reads as `RETURNED` at delivery.

The nested turn follows its five rules. `drop-subtask.wast` and
`subtask-wait.wast` prove that a yielded item runs. `deadlock.wast`,
`wait-forever.wast`, `wait-forever2.wast`, `backpressure-deadlock.wast`, and the
eighth case of the Component Model `reentrance.wast` prove the deadlock cause on
idle. The second directive of `dont-block-start.wast` proves the cannot-block
cause. Repository tests prove the stack-switch cause with a host task pending,
that a sync-typed task's block runs only items of its own instance, and that a
nested turn can open another.

The seam's budget ends a wait the store never serves. `async-calls-sync.wast`
and `reenter-during-yield.wast` reach it from their two shapes, the spin-waiting
callee and the yield loop, and both fail with the stack-switch message.
Repository tests prove each boundary: a block whose turns run nothing but one
resumption fails past the budget and one whose turns run work of their own is
served to the end, a thread that gives way the budget's worth of times sees zero
from every yield, and the yield past the budget ends the call it is inside.

A synchronous lower returns at `task.return`. `callback-yield-then-exit.wast`
passes, and the callee's late exit runs in the next driver.

Reentrance does not trap. The eight owned directives of the Component Model
`reentrance.wast` pass, the five synchronous ones through the adapters of
Wasmtime 49 alone.

An exception reaches the host. `exceptions.wast` passes whole.

Two host calls overlap. Repository tests prove that two `call_concurrent`
futures into one callback export interleave and both resolve, that a call into a
synchronous export runs while a callback task waits, that a second synchronous
call is queued behind a first, that a dropped future cancels nothing, and that
an idle store leaves the entry pending.

The accessor is a token. Repository tests prove that `with` reaches the host
data during a poll, that `with` inside `with` fails with the recursive-driver
cause, that `with` outside a poll and with another store's accessor fail with
the store-not-in-poll cause, and that a host task's future holds the accessor
across an await. In the browser a host `async` function awaits a JavaScript
promise and returns its value to the guest.

The corpus lines are removed. The expected-failure list loses every line the
owned files and directives above account for, and no other line. The progress
summary shows the two `async` rows with the new counts, the same on both
targets.

Every facet above holds on both targets. The native run and the browser run
report the same pass and failure results for every named file and every
repository test.

## References

- [PDD006], component parsing, whose translator emits the adapter trampolines.
- [PDD007], linking and instantiation, whose resolver enforces the link rule.
- [PDD008], the canonical ABI, whose flattening the synchronous lower keeps.
- [PDD010], the typed call surface.
- [PDD014], borrow lifetime tracking, whose lend rule the subtask applies.
- [PDD016], the conformance suite.
- [PDD018], the concurrency runtime model. This document revises its nested turn
  and the shape of its accessor.
- [PDD019], tasks and the callback export, whose status words and waitable sets
  this design reuses.
- [Concurrency], the Concurrency explainer, and its sections on [subtasks and
  supertasks][Subtasks], [blocking][Blocking], [borrows][Borrows],
  [reentrance][Reentrance], and the [async import ABI][Async Import ABI].
- [CanonicalABI – canon lower], the reference's lowering, with [subtask
  state][CanonicalABI – subtask state] and
  [`subtask.drop`][CanonicalABI – subtask.drop].
- [`definitions.py`], the executable reference for `canon_lower`, `Subtask`,
  `Task.enter_implicit_thread`, and `canon_subtask_drop`.
- [Wasmtime], the reference implementation at `v49.0.0-rc.1`: its [concurrent
  runtime][Wasmtime concurrent], its [fused adapter compiler][Wasmtime fact],
  its [host function registration][Wasmtime linker], its [registration type
  check][Wasmtime host], its [trap messages][Wasmtime traps], and its [component
  spectest][Wasmtime spectest].
- The Wasmtime API for [`Accessor`], [`run_concurrent`], [`call_concurrent`],
  and [`func_wrap_concurrent`].
- The [Component Model test corpus] and the [Wasmtime component tests].

[PDD006]: ./PDD006%20Component%20Parsing.md
[PDD007]: ./PDD007%20Linking%20and%20Instantiation.md
[PDD008]: ./PDD008%20Canonical%20ABI%20and%20Host%20Functions.md
[PDD010]: ./PDD010%20Pre-wasip3%20Public%20API.md
[PDD014]: ./PDD014%20Borrow%20Lifetime%20Tracking.md
[PDD016]: ./PDD016%20Conformance%20Suite.md
[PDD018]: ./PDD018%20Concurrency%20Runtime%20Model.md
[PDD019]: ./PDD019%20Tasks%20and%20the%20Callback%20Async%20Export.md
[Concurrency]:
  https://github.com/WebAssembly/component-model/blob/main/design/mvp/Concurrency.md
[Subtasks]:
  https://github.com/WebAssembly/component-model/blob/main/design/mvp/Concurrency.md#subtasks-and-supertasks
[Blocking]:
  https://github.com/WebAssembly/component-model/blob/main/design/mvp/Concurrency.md#blocking
[Borrows]:
  https://github.com/WebAssembly/component-model/blob/main/design/mvp/Concurrency.md#borrows
[Reentrance]:
  https://github.com/WebAssembly/component-model/blob/main/design/mvp/Concurrency.md#reentrance
[Async Import ABI]:
  https://github.com/WebAssembly/component-model/blob/main/design/mvp/Concurrency.md#async-import-abi
[CanonicalABI – canon lower]:
  https://github.com/WebAssembly/component-model/blob/main/design/mvp/CanonicalABI.md#canon-lower
[CanonicalABI – subtask state]:
  https://github.com/WebAssembly/component-model/blob/main/design/mvp/CanonicalABI.md#subtask-state
[CanonicalABI – subtask.drop]:
  https://github.com/WebAssembly/component-model/blob/main/design/mvp/CanonicalABI.md#-canon-subtaskdrop
[`definitions.py`]:
  https://github.com/WebAssembly/component-model/blob/main/design/mvp/canonical-abi/definitions.py
[Wasmtime]: https://github.com/bytecodealliance/wasmtime
[Wasmtime concurrent]:
  https://github.com/bytecodealliance/wasmtime/blob/v49.0.0-rc.1/crates/wasmtime/src/runtime/component/concurrent.rs
[Wasmtime fact]:
  https://github.com/bytecodealliance/wasmtime/blob/v49.0.0-rc.1/crates/environ/src/fact/trampoline.rs
[Wasmtime linker]:
  https://github.com/bytecodealliance/wasmtime/blob/v49.0.0-rc.1/crates/wasmtime/src/runtime/component/linker.rs
[Wasmtime host]:
  https://github.com/bytecodealliance/wasmtime/blob/v49.0.0-rc.1/crates/wasmtime/src/runtime/component/func/host.rs
[Wasmtime traps]:
  https://github.com/bytecodealliance/wasmtime/blob/v49.0.0-rc.1/crates/environ/src/trap_encoding.rs
[Wasmtime spectest]:
  https://github.com/bytecodealliance/wasmtime/blob/v49.0.0-rc.1/crates/wast/src/spectest.rs
[`Accessor`]:
  https://docs.wasmtime.dev/api/wasmtime/component/struct.Accessor.html
[`run_concurrent`]:
  https://docs.wasmtime.dev/api/wasmtime/struct.StoreContextMut.html#method.run_concurrent
[`call_concurrent`]:
  https://docs.wasmtime.dev/api/wasmtime/component/struct.Func.html#method.call_concurrent
[`func_wrap_concurrent`]:
  https://docs.wasmtime.dev/api/wasmtime/component/struct.LinkerInstance.html#method.func_wrap_concurrent
[Component Model test corpus]:
  https://github.com/WebAssembly/component-model/tree/main/test/async
[Wasmtime component tests]:
  https://github.com/bytecodealliance/wasmtime/tree/main/tests/misc_testsuite/component-model/async
