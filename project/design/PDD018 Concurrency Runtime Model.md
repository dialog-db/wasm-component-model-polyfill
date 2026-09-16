# Concurrency Runtime Model

[PDD005] made every guest entry point an `async fn` so that the concurrency
features of the Component Model can sit on top of the synchronous baseline.
[PDD006] through [PDD017] built that baseline. The translator of [PDD006]
already parses every concurrency built-in, and the polyfill rejects each one at
one place with `Error::Unsupported`. This document designs the runtime those
built-ins need. It settles the parts that tasks, subtasks, waitables, streams,
and futures share. Those parts are the scheduler, the handle table, the task
records, and the host task contract. They also include the suspend seam, the
lift and lower seam, and the scheduling order. It designs no built-in. Each
concurrency feature adds its built-ins on this model.

The model follows the [Concurrency explainer][Concurrency] and the Python
reference in [`definitions.py`]. Where the reference leaves a choice to the
host, the model makes the choice [Wasmtime] makes, so that the conformance
corpora of [PDD016] match. Where the model departs from either, this document
states the reason.

## Goals

- One cooperative scheduler per `Store` runs every task of every instance in the
  store. Guest code runs only inside a turn of that scheduler. A turn is one
  poll of the scheduler by the host.
- The host drives the scheduler through the futures it already awaits.
  `Func::call` keeps its shape. A second entry runs the scheduler for work that
  no call owns.
- Each component instance keeps one handle table for every handle kind, as the
  runtime-state rules of the Canonical ABI require.
- A task record replaces the per-call scope of [PDD014]. A synchronous call is a
  task with one thread.
- A host `async` function is a future the store owns and polls. The contract
  fixes how the future reaches the store and from which lowered form a guest can
  call it.
- The scheduler has one seam for suspending a guest thread. The callback form,
  where an `async` export returns a status code instead of blocking, never uses
  the seam. A target that can switch stacks fills the seam without a change to
  the callback path.
- Every value that crosses the boundary passes through one context. The context
  hides how guest memory is read, written, and allocated. A second ABI can then
  sit beside the eager one, which stores values into linear memory.
- The scheduler resolves every choice the specification leaves open the way
  Wasmtime resolves it. The order is the same on both targets.
- The `async` directories of both conformance corpora are vendored before any
  concurrency feature lands. Every failing directive is recorded as a deferred
  feature.

## Non-goals

- The concurrency built-ins as trampolines: `task.*`, `waitable-set.*`,
  `waitable.join`, `subtask.*`, `backpressure.*`, `context.*`, `stream.*`,
  `future.*`, `error-context.*`, and the thread built-ins. The model states what
  each one does to the records it defines. It builds none of them.
- The callback form of `canon lift async`, and the status words of its protocol
  as an implementation.
- The asynchronous `canon lower`, and the prepare-and-start call protocol of the
  adapter modules.
- Streams, futures, and error contexts as value types, and their handle kinds
  beyond a slot in the table.
- Cancellation delivery, host-side cancellation of a task, and the rules that
  decide which trap poisons an instance.
- The stackful form of `canon lift async`, JavaScript Promise Integration, a
  native fiber, and every other provider of the suspend seam.
- The registration entry point for a host `async` function, typed and untyped.
  The entry point that starts several host calls into one instance at the same
  time.
- The GC data model and the lazy ABI. The lift and lower seam admits them.
  Nothing implements them.

## The Scheduler and Its Drivers

The scheduler is a cooperative loop that the store owns. It holds the ready
queues, the host tasks, and the records of every task. It has no thread of its
own and no executor of its own. A driver is a host future that polls the
scheduler. One poll of a driver is a turn. A turn runs guest work that is ready,
polls the host tasks the executor woke, and returns to the driver when nothing
is ready. Guest code runs only inside a turn, or inside the resumption of a
guest thread that the scheduler suspended through the suspend seam. Nothing else
calls into a guest.

Three drivers exist:

- `Func::call`, and the typed call of [PDD010]. The future creates a task for
  the export, polls the scheduler until the task resolves, and returns the
  lifted result. A task of a synchronous export resolves when its core function
  returns. A task of an `async` export resolves at `task.return`. Work the task
  leaves behind after it resolves stays in the store. The future does not wait
  for it.
- `Linker::instantiate` of [PDD007]. Instantiation runs the initializers of the
  plan and the core `start` functions inside turns.
- A store entry named after Wasmtime's `run_concurrent`. It takes a closure,
  hands the closure an accessor to the store, and polls the scheduler until the
  closure's future completes. The accessor is the one the host task contract
  defines below. A host uses this entry to let a task finish after its call
  returned, and to run host tasks that no call owns. The entry also exists so
  that several host calls into one instance can run at the same time. The call
  entry that starts such a call is out of scope.

Four rules hold for every driver:

- A driver entered while another driver of the same store is inside a turn fails
  with the scheduler error defined under Error Model Growth. Wasmtime refuses a
  recursive `run_concurrent` for the same reason. A nested turn, which the
  suspend seam defines, is not a driver. This rule does not apply to it.
- When a turn finds nothing ready, no host task pending, and the driver's
  condition unmet, `Func::call` and instantiation fail with Wasmtime's deadlock
  trap. If the waiting task is one that must not block, the failure is
  Wasmtime's cannot-block trap instead. The `run_concurrent` entry returns
  pending in that state, because its closure can wait on something outside the
  store.
- Dropping a driver's future cancels nothing. The task stays in the store and
  runs in the next turn of any driver. This is Wasmtime's rule for
  `call_concurrent`.
- Dropping the store drops every task, host task, and suspended thread. No
  destructor runs, per [PDD017].

The same scheduler code runs on both targets. Natively, any executor polls the
driver. In the browser, `wasm-bindgen-futures` polls it. The scheduler takes one
thing from the executor: the waker in the driver's context. The scheduler
records that waker for the duration of the turn, so that a trampoline can poll a
host task with it.

```text
fn turn(scheduler, waker) -> Outcome:
    scheduler.active_waker = waker
    if scheduler.resume_after_yield is set:
        run(scheduler.resume_after_yield.take())
    loop:
        if scheduler.switch_slot is set:
            run(scheduler.switch_slot.take())
        else if scheduler.high_priority is not empty:
            run(scheduler.high_priority.pop_front())
        else if scheduler.low_priority is not empty:
            scheduler.resume_after_yield = scheduler.low_priority.pop_front()
            return Yield            // the driver wakes itself and returns pending
        else:
            break
    for host_task in scheduler.woken_host_tasks():
        poll(host_task, waker)      // a completed host task queues its lowering
    scheduler.active_waker = none
    if scheduler has a ready item:
        return Progress
    if scheduler.host_tasks is empty:
        return Idle
    return Waiting
```

`run` executes one item. An item is the start of a task, a callback invocation,
the resumption of a thread, or the lowering of a completed host task's result.
An item runs to its next yield point and returns. A host task that joined the
store since the last turn counts as woken. The driver loops on `Progress`,
returns pending on `Waiting` and `Yield`, and applies the idle rule on `Idle`.

## The Handle Table

Each component instance keeps one handle table. The table is the `handles` table
of the reference's `ComponentInstance`. Every handle a guest holds is an index
into it, whatever the kind. This document revises the table shape of [PDD009],
which kept one table per resource type per store. It also revises the transfer
rule of [PDD015], which relied on that shape.

An entry has one of these kinds:

- An owned resource: the resource type identity, the rep, and the count of
  borrows lent from it.
- A borrowed resource: the type identity, the rep, and the task the borrow is
  owed to.
- A subtask: the index of the subtask record in the store.
- A waitable set: the index of the set record in the store.
- A readable or writable stream end, a readable or writable future end, and an
  error context. These kinds are reserved here and defined by the features that
  add them.

Index zero is never allocated. A freed index returns to a free list, and reuse
is deterministic within one instance, as [PDD009] states for its tables. A lift
of a resource handle compares the type identity on the entry with the declared
type, as today. A transfer of an owned handle between two instances removes the
entry from the source table and inserts it into the destination table. The index
changes. A transfer of a borrow inserts a borrow entry into the destination
table for the duration of the call.

The store keeps a separate host table for the handles the host holds. Those are
the handles it minted through `Store::resource_new` and the owned handles it
lifted out of a result. The host table is not an instance table. Its indices
never reach a guest.

## Tasks, Subtasks, and Threads

A task is the record of one call into an export. A subtask is the record of one
call out through an import. A thread is one guest execution, and every task has
at least one, its implicit thread. The store keeps one table of task records,
one of subtask records, and one of thread records. A handle table entry points
into those tables by index.

A task record holds:

- The function type of the export and the canon options of its lift.
- The instance the export belongs to.
- The state: `initial`, `started`, `pending-cancel`, `cancel-delivered`, or
  `resolved`.
- The count of borrows the task received and has not yet seen dropped, named
  `num_borrows` in the reference.
- The implicit thread, and the list of every thread the task contains.
- The result once the task returned it, or the caller's channel that receives
  the result.

A subtask record holds:

- The state: `starting`, `started`, `returned`, `cancelled-before-started`, or
  `cancelled-before-returned`. The last three are the resolved states.
- The list of handles the caller lent for the call, named `lenders` in the
  reference. The count on each lent handle is decremented when the resolution is
  delivered. A resolution is delivered when the caller's thread receives the
  subtask event, or when a synchronous lower returns.
- The pending event, the waitable set the subtask joined, and the flag that
  marks a synchronous waiter.
- Whether cancellation was requested.

A thread record holds the task that contains it and the readiness condition it
waits on. It also holds the two context slots that `context.get` and
`context.set` read and write. The context slots move from one pair per
instantiation to one pair per thread.

The store keeps a stack of current scopes in place of the call-scope stack of
[PDD014]. A scope is a task record or a subtask record. The top of the stack is
the current scope. When it is a task, that task is the current task, and its
running thread is the current thread. The stack is still a stack because
synchronous calls still nest on the one real stack. A host call into an export
pushes the export's task. A guest call into a host function pushes the subtask
of that call, and the subtask stays on the stack while the host side runs. An
adapter's enter intrinsic pushes the callee's task, and its exit intrinsic pops
it. A synchronous call between two components is therefore a task with one
thread. A synchronous export called from the host is the same. The synchronous
baseline is the case of one task per instance at a time.

Every borrow operation of [PDD014] consults the current scope. A borrow lowered
into a guest increments the borrow count of the current task, and the table
entry records that task. A drop of the borrow decrements the count of the task
the entry names. A borrow lifted from an owned handle adds the owned handle to
the lender list of the current subtask. `task.return`, `task.cancel`, and the
return of a synchronous export each trap when the task's borrow count is not
zero. That rule is the scope-exit rule of [PDD014] under its new name.

Each instance keeps an instance record with the fields of the reference's
`ComponentInstance` that the runtime needs. Those are the `backpressure`
counter, the count of tasks waiting to enter, the exclusive thread, the
`may_leave` flag, and a may-not-suspend flag. The flags global that [PDD015]
gives each instance for its adapters stays as the adapters see it. The enter
intrinsic sets the may-not-suspend flag on the callee instance for the duration
of a synchronous call. The exit intrinsic restores it, as Wasmtime does.

A task of an `async` export passes the entry gate before its thread runs. The
gate is the reference's `enter_implicit_thread`:

```text
fn enter(task) -> bool:
    inst = task.instance
    needs_exclusive = not task.options.async or task.options.callback
    fn blocked():
        return inst.backpressure > 0
            or (needs_exclusive and inst.exclusive_thread is set)
    if blocked() or inst.waiting_to_enter > 0:
        inst.waiting_to_enter += 1
        wait until not blocked()       // the task is queued, the thread does not run
        inst.waiting_to_enter -= 1
        if task.deliver_pending_cancel():
            task.cancel()
            return false
    if needs_exclusive:
        inst.exclusive_thread = task.implicit_thread
    return true
```

A task of a synchronous export ignores the gate, as the reference states. A task
that waits at the gate is a queued item, not a suspended thread, so the gate
needs no stack switch.

## Waitables and Events

A waitable is a handle a guest can wait on. The kinds are a subtask, a readable
stream end, a writable stream end, a readable future end, and a writable future
end. Every waitable record holds one pending event slot, the waitable set it
joined if any, and a flag that marks a synchronous waiter. A waitable set holds
the list of its waitables and the count of threads waiting on it.

An event is a triple: a code, and two payloads. The codes are those of the
reference's `EventCode`. They are none (0), subtask (1), stream read (2), stream
write (3), future read (4), future write (5), and task cancelled (6). For a
subtask event the payloads are the subtask's index in the handle table and its
state. For a copy event they are the waitable's index and the copy result. The
scheduler records readiness by filling a waitable's pending event slot. It
delivers an event when a thread waits on or polls a set that contains the
waitable. It also delivers one when a callback returns the wait code with that
set. Delivery empties the slot.

The rules of the reference hold at the record level:

- A set delivers events in the order its waitables joined it.
- A wait on a set that already holds an event returns at once.
- Joining a waitable to a set removes it from its previous set. Joining a
  waitable that has a synchronous waiter traps.
- Dropping a set that still holds waitables traps, and so does dropping a set a
  thread is waiting on.
- Dropping a subtask whose resolution was not delivered traps.

## Host Tasks

A host task is the future that one call of a host `async` function produces. The
runtime layer gives a host trampoline a synchronous closure and nothing else, so
the trampoline cannot run the future. It hands the future to the scheduler and
returns to the guest. The status word an asynchronous lower returns to the guest
carries the subtask state in its low four bits and the subtask index above them.
The contract:

- The future is `'static`. It does not borrow the store. It reaches the host
  data of the store only through an accessor, and only inside a closure the
  accessor runs during a poll. The accessor mirrors Wasmtime's `Accessor`. A
  value taken from the host data must be cloned out of the closure. An accessor
  used inside another accessor's closure fails with the scheduler error.
- The future must be `Send` on the native target, so that `Store<T>` stays
  `Send` as it is today. The bound is absent in the browser. A JavaScript
  promise wrapped as a future is not `Send`, and awaiting one is the purpose of
  a browser host function. The polyfill expresses the bound as one trait whose
  definition differs per target, so host code compiles on both targets without
  change.
- The trampoline polls the future once before it returns to the guest. It uses
  the waker of the active turn, or a waker that does nothing when no turn is
  active. If the future is ready, the result lowers at once and the guest sees
  the `RETURNED` status with no subtask. If the future is pending, it joins the
  store's host tasks. A subtask enters the caller's handle table in the
  `started` state, and the guest sees `STARTED` with that index. The next turn
  polls the future again with the driver's waker, so no wake is lost.
- When the future completes in a later turn, the scheduler lowers its result
  through the boundary context of the subtask. It then moves the subtask to
  `returned` and fills its pending event with a subtask event.
- A guest can call a host `async` function through an asynchronous lower always.
  Through a synchronous lower, the call succeeds if the first poll resolves the
  future, or if the suspend seam is filled on this target. Otherwise the
  trampoline fails with the scheduler error's stack-switch cause.
- A synchronous host function called through an asynchronous lower returns
  `RETURNED` at once. That path exists today and keeps its behavior.

```text
fn call_host_async(trampoline, args) -> status:
    subtask = Subtask::new()
    future = trampoline.registration.start(args)
    waker = scheduler.active_waker or noop_waker
    match poll(future, waker):
        Ready(result):
            lower(subtask.context(), result)
            return RETURNED
        Pending:
            scheduler.host_tasks.push(future, subtask)
            index = current_instance().handles.insert(Subtask(subtask))
            subtask.state = started
            return STARTED | (index << 4)
```

## The Suspend Seam

The reference lets a running guest thread block inside a built-in. The blocking
built-ins are:

- A synchronous `waitable-set.wait`.
- A synchronous `stream.read` or `future.read` that is not ready.
- A synchronous lower of an `async` callee that blocks.
- A synchronous `subtask.cancel`.
- `thread.suspend`.

Wasmtime serves each one by suspending the fiber the guest runs on. The polyfill
runs the guest on the one real stack. A host trampoline that must block
therefore has no way back to the scheduler without unwinding the guest.

The callback form never needs this. A callback task blocks by returning the
`WAIT` or `YIELD` code to the polyfill, which is a plain return. A
`waitable-set.poll` never blocks. A `thread.yield` can return at once, which the
reference permits. The full callback protocol therefore runs with no stack
switch, and it is the baseline on every target.

The scheduler names one suspend capability for the cases that remain. A blocking
built-in asks the scheduler to suspend the current guest thread until a
readiness condition holds. When the target fills the capability, the thread
suspends and the turn ends. In the browser, JavaScript Promise Integration
([JSPI]) is the intended provider. Under JSPI the host's entry into the guest
becomes a promising call, which returns a promise when the guest suspends. A
blocking built-in becomes a suspending import, a host function whose promise
suspends the guest until it resolves. The native target has no provider in this
design. Neither provider is designed here.

When the capability is absent, a blocking built-in runs a nested turn from
inside the guest call. The nested turn runs ready guest work in other tasks. It
polls the host tasks the executor woke, with the waker of the outer turn, until
the condition holds or nothing can progress. A host task that stays pending
inside a nested turn stays in the store for the outer turn, so no wake is lost.
If the nested turn goes idle with the condition unmet, the built-in traps. A
task that must not block gets Wasmtime's cannot-block trap, which is the rule of
the reference. A task that is allowed to block gets the scheduler error's
stack-switch cause. The reference permits that block, and only the target cannot
serve it. A nested executor that blocks the native thread is ruled out. It
deadlocks under a current-thread executor, tokio forbids it inside a runtime,
and it has no browser counterpart.

Two rules of JSPI shape the model now, so that a provider fits later:

- A suspension traps if any frame that is not WebAssembly sits between the
  promising entry and the suspending import. In the browser the polyfill is
  itself WebAssembly behind JavaScript glue, so every host trampoline puts a
  JavaScript frame on the stack. When the capability is present, guest code is
  therefore entered only from the scheduler, never from inside a trampoline. The
  one exception is a frame that cannot block: a synchronous resource destructor
  or a `post-return` run from a trampoline. The nested turn runs only when the
  capability is absent, so the two never meet.
- A suspended guest resumes on a microtask, outside any poll of a driver. A
  microtask runs before the browser returns to its event loop, and a macrotask
  runs after. This is why a turn is defined as a poll of a driver or the
  resumption of a suspended thread. It is also why the scheduler's state is
  reachable from a trampoline without a driver on the stack.

## The Boundary Context

A boundary context is the object through which one value crosses between the
host's `Val` and the guest's memory or flat slots. It is the reference's
`LiftLowerContext`. One context is built per crossing from three things. Those
are the canon options of the lift or lower, the component instance, and the task
or subtask whose borrows the crossing counts against. Every crossing uses one:

- Arguments and results, in both directions.
- The parameters of `task.return`.
- The result written to the out-pointer of an asynchronous lower.
- The element payloads that streams and futures copy, under the options of the
  copy.

The context is the only object that reads guest memory, writes guest memory, or
asks the guest for memory. Nothing outside the lift and lower code names
`cabi_realloc`, a memory, or a byte offset. A trampoline, an intrinsic, or the
scheduler hands the context a value, a type, and a position. It reads back a
value or a list of flat slots.

The context selects an ABI strategy from its options. The eager strategy is the
only one this design implements. It stores a value into linear memory the caller
supplied or `cabi_realloc` returned, and it loads a value from a pointer. The
data model of the options, linear memory today, is part of the selection. A
second strategy, lazy or GC, sits beside the eager one behind the same context.
An option the polyfill does not implement fails at translation with
`Error::Unsupported`, as now.

The context carries its options as a value. `task.return` must make sure that
its options equal the lift options of the task. It must also make sure that its
result type equals the result type of the task's function. Both live on the
context.

```text
struct BoundaryContext:
    options: CanonOptions         // memory, realloc, string encoding, data model
    instance: InstanceId
    scope: Task | Subtask         // where borrows and lends are counted
    strategy: AbiStrategy         // eager linear memory today

fn lower(cx, value, ty, position) -> FlatSlots
fn lift(cx, slots_or_ptr, ty, position) -> Val
```

## Scheduling Order

The reference is nondeterministic at six points and lets the host resolve each
one. The polyfill resolves them as Wasmtime does, and the order is identical on
both targets:

- The scheduler has three queues. One slot holds a thread it must switch to
  next, which is the callee thread of a call between two components. A
  high-priority queue holds fresh readiness. A low-priority queue holds
  resumptions after a yield.
- A set delivers pending events in the order its waitables joined it.
- A wait whose set already holds an event returns without blocking.
- Threads that became ready together resume in the order they became ready.
  Tasks held at the entry gate start in arrival order when the gate opens.
- A yield, whether `thread.yield` or a callback's `YIELD` code, always gives
  way. The task resumes after every other ready item, and its resumption first
  returns control to the host executor.

Returning control to the host executor is the one place where the targets
differ. Natively the driver wakes itself and returns pending, which lets the
executor run its own timers and sockets. In the browser the driver is polled
from a microtask. A self-wake lands back in the microtask queue, ahead of every
network response and timer. A guest that spins on `thread.yield` and
`waitable-set.poll` then starves the page. The browser's yield therefore crosses
a macrotask boundary before it wakes the driver. The scheduler code is shared.
Only the wake after a yield is per target.

## Target Differences

Four things differ between the native target and the browser. Everything else in
this document is the same on both:

- Who polls the driver: any executor natively, `wasm-bindgen-futures` in the
  browser.
- The `Send` bound on a host task: required natively, absent in the browser.
- The wake after a yield: a self-wake natively, a macrotask hop in the browser.
- The intended provider of the suspend capability: none natively, JSPI in the
  browsers that ship it. This design fills the capability on neither target. The
  callback form is the baseline in every browser.

## Error Model Growth

`wcmp::Error` gains one variant for the scheduler, with a structured cause:

- Deadlock: a driver went idle with its condition unmet. The message is the one
  Wasmtime prints for its deadlock trap.
- Cannot block: a task that must not block went idle. The message is Wasmtime's
  cannot-block trap.
- Recursive driver: a driver was entered while another was inside a turn, or an
  accessor was used inside another accessor's closure.
- Stack switch needed: a guest thread blocked where the reference permits it and
  the target has no suspend provider. This cause is not `Error::Unsupported`.
  The feature is supported and only the capability is missing, and a host wants
  to branch on it.

[PDD005]'s note about `anyhow::Error` applies.

## The Corpus Baseline

The `async` directories of both corpora of [PDD016] are vendored at the upstream
commits the corpus already records. Those are the `async` directory of the
Component Model test corpus and the `async` directory of the Wasmtime component
tests. Every directive that fails is recorded in the expected-failure list as a
deferred feature. A directive that passes already is not listed and counts as a
pass. The progress summary gains one row per directory.

The five asynchronous host items of Wasmtime's component spectest need a host
task registration: `never-return`, `return-two-slowly`, `echo-slowly`,
`[method]resource1.never-return`, and `host-echo-u32`. `host-echo-u32` is
registered as a synchronous host function now. A synchronous host function
called through an asynchronous lower returns `RETURNED` at once, which is the
behavior the spectest wants. Until the harness has a host task registration,
every directive whose component imports one of the other four is a deferred
feature. Its line names that reason.

The vendoring is complete before any concurrency feature lands, so that the
first feature measures against a baseline instead of creating one. Each feature
then shows its progress by removing lines. The two-way comparison of the list
enforces this: a listed directive that starts to pass fails the run until its
line is removed.

## User Stories

A developer runs a component in a browser page and calls an `async` export.

> The developer awaits the call as a JavaScript promise. Under the promise, the
> scheduler runs the guest's task in turns. The page stays responsive while the
> task waits, and the promise resolves with the result when the task returns.

A developer registers a host function that fetches over the network and hands
the response to a guest.

> The function returns a future that awaits the browser's fetch promise. The
> trampoline polls it once, sees it pending, and gives the guest a subtask. The
> guest waits on it. When the response arrives, a turn lowers it and delivers
> the subtask event.

A contributor builds the first concurrency feature.

> The contributor finds the task record, the entry gate, the handle table, the
> waitable records, and the boundary context in place. The contributor adds the
> built-ins as trampolines that mutate them.

A contributor adds a stack switch to one target.

> The contributor fills the suspend capability. The callback path does not
> change, and the nested turn stops being the fallback on that target.

A reviewer reads the conformance summary after a feature lands.

> The reviewer sees the two `async` rows, the lines the feature removed, and the
> same counts on both targets.

## Test Cases

A synchronous call runs as a task. A host call into a synchronous export pushes
a task on the current-task stack and pops it on return. A guest call into a
synchronous host function and a synchronous call between two composed components
do the same. Every borrow test of the synchronous baseline passes unchanged,
including `borrows.wast`, `handle-table.wast`, and `multiple-resources.wast` of
the Component Model corpus and `resources.wast` of the Wasmtime corpus.

The handle table is per instance. Two handles of different resource types in one
instance take consecutive indices from one allocator. An owned handle
transferred between two composed instances leaves the source table and takes an
index in the destination table. The `linking` directory of the Component Model
corpus, the composition tests of the Wasmtime corpus, and the fixtures in the
repository pass unchanged.

The driver entry runs a closure. The `run_concurrent` entry runs its closure
with an accessor that reaches the store's host data and returns the closure's
value. It refuses a nested driver with the recursive-driver cause.

The boundary context is the one crossing. No call site outside the lift and
lower code names `cabi_realloc` or reads or writes a guest memory. Every export
call, host trampoline, and adapter intrinsic builds one context from options,
instance, and task.

The async corpora are vendored. The progress summary shows one row for each
`async` directory with its directive count and its deferred-feature count. The
expected-failure list has one line per failing directive. `host-echo-u32` is
registered, and every directive that imports one of the other four asynchronous
spectest items is listed as a deferred feature.

Every facet above holds on both targets. The native run and the browser run
report the same counts for the `async` rows. They report the same pass and
failure results for every named file.

## References

- [PDD005], the foundations and posture, whose `async fn` rule this design
  fulfills.
- [PDD007], linking and instantiation.
- [PDD009], resources. This document revises its table shape.
- [PDD010], the typed call surface.
- [PDD014], borrow lifetime tracking. This document moves its scope onto the
  task record.
- [PDD015], component composition. This document revises its transfer rule.
- [PDD016], the conformance suite.
- [PDD017], resource disposal.
- [Concurrency], the Concurrency explainer.
- [CanonicalABI – runtime state], the handle table rules.
- [`definitions.py`], the executable reference for tasks, subtasks, waitables,
  and the entry gate.
- [Wasmtime], the reference implementation, and its [`run_concurrent`],
  [`Accessor`], and [`func_wrap_concurrent`] surface.
- [JSPI], the JavaScript Promise Integration proposal, and the rules it places
  on frames between a promising export and a suspending import.
- The [Component Model test corpus] and the [Wasmtime component tests].

[PDD005]: ./PDD005%20Library%20Foundations.md
[PDD006]: ./PDD006%20Component%20Parsing.md
[PDD007]: ./PDD007%20Linking%20and%20Instantiation.md
[PDD009]: ./PDD009%20Resources.md
[PDD010]: ./PDD010%20Pre-wasip3%20Public%20API.md
[PDD014]: ./PDD014%20Borrow%20Lifetime%20Tracking.md
[PDD015]: ./PDD015%20Component%20Composition.md
[PDD016]: ./PDD016%20Conformance%20Suite.md
[PDD017]: ./PDD017%20Resource%20Disposal.md
[Concurrency]:
  https://github.com/WebAssembly/component-model/blob/main/design/mvp/Concurrency.md
[CanonicalABI – runtime state]:
  https://github.com/WebAssembly/component-model/blob/main/design/mvp/CanonicalABI.md#runtime-state
[`definitions.py`]:
  https://github.com/WebAssembly/component-model/blob/main/design/mvp/canonical-abi/definitions.py
[Wasmtime]: https://github.com/bytecodealliance/wasmtime
[`run_concurrent`]:
  https://docs.wasmtime.dev/api/wasmtime/struct.StoreContextMut.html#method.run_concurrent
[`Accessor`]:
  https://docs.wasmtime.dev/api/wasmtime/component/struct.Accessor.html
[`func_wrap_concurrent`]:
  https://docs.wasmtime.dev/api/wasmtime/component/struct.LinkerInstance.html#method.func_wrap_concurrent
[JSPI]: https://github.com/WebAssembly/js-promise-integration
[Component Model test corpus]:
  https://github.com/WebAssembly/component-model/tree/main/test/async
[Wasmtime component tests]:
  https://github.com/bytecodealliance/wasmtime/tree/main/tests/misc_testsuite/component-model/async
