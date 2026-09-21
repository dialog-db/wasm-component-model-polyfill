# Tasks and the Callback Async Export

[PDD018] designed the runtime that every concurrency feature shares: the
scheduler and its drivers, the handle table, the task, subtask, and thread
records, the waitable records, the host task contract, the suspend seam, and the
boundary context. It built no built-in. This document designs the first
concurrency feature on that model. The feature is an export lifted with
`canon lift async` and a `callback`, called from the host. It comes with the
task built-ins that such an export uses. Those built-ins are `task.return`,
`backpressure.inc`, `backpressure.dec`, `waitable-set.new`, `waitable-set.wait`,
`waitable-set.poll`, `waitable-set.drop`, `waitable.join`, `thread.yield`, and
the two context slots. Every one of them works from every task, so a synchronous
export gains them too.

A callback export is an export whose lift carries the `async` option and a
`callback` function. The Concurrency explainer names this form the stackless
async export. The polyfill runs the guest on the one real stack. The callback
form never needs a stack switch, which is why it is the first feature. The
design follows the [Concurrency explainer][Concurrency] and the Python reference
in [`definitions.py`]. Where the reference leaves a choice to the host, the
design makes the choice [Wasmtime] makes at `v49.0.0-rc.1`. That tag is the
first to carry Wasmtime's realignment with the current reference, and its trap
messages equal those of the version before it. The design assumes the fused
adapter compiler of Wasmtime 49, whose synchronous adapters import no may-block
global and manage the context slots around each realloc call themselves. Where
the design departs from either, it states the reason.

## Goals

- The translator accepts a callback export and records its callback function. A
  host calls the export through `Func::call` and the typed call of [PDD010], and
  both keep their shape.
- A call into a callback export is a task. Its implicit thread runs the export
  and then its callback, once per event, until the callback exits. The call's
  future resolves when the task returns its result through `task.return`.
- The three status words of the callback protocol, exit, yield, and wait, have
  the effect the reference gives them. The event the callback receives on resume
  is the one the reference delivers.
- `task.return` lifts the result through one boundary context and traps on the
  conditions of the reference.
- Backpressure is a per-instance counter with the reference's range, and it
  gates the start of an asynchronous task as the entry gate of [PDD018] states.
- A waitable set is a handle a single task creates, waits on, polls, joins a
  waitable to, and drops, with the traps of the reference.
- The context slots belong to the thread. They start at zero, survive every
  resumption of the callback, and end with the thread.
- A destructor that `resource.drop` runs is a task with one thread, as the
  reference lifts it.
- The public function type states whether the function is `async`, spelled as
  Wasmtime spells it.
- Every trap the feature adds is a structured error cause with Wasmtime's
  message, so the conformance corpora of [PDD016] match it by substring.
- The behavior is the same on both targets.

## Non-goals

- A call into an asynchronous export from a sibling component, through a
  synchronous or an asynchronous lower. Wasmtime compiles both through the
  prepare-and-start adapter protocol, which this document does not design. The
  corpus files that need it stay deferred with that reason.
- The asynchronous `canon lower`, the subtask built-ins, and an asynchronous
  host function. A host task never starts in this design, so no event other than
  the none event ever reaches a callback in the corpus.
- A second host call into an instance while a first call's task is still
  running. `Func::call` and the typed call are the only host entries into an
  export, and one runs at a time. The entry gate and the exclusive thread are
  designed here and observed through backpressure and through tests that queue
  two tasks through the store's records.
- Streams, futures, and error contexts. The two files the corpus offers for an
  empty wait and a wait inside a callback need future built-ins and stay
  deferred.
- Cancellation. `task.cancel` stays unsupported, the cancelled event is never
  delivered, and the two cancel states of the task record are never entered.
- The stackful form of `canon lift async`, which has no `callback`. It stays
  unsupported at translation.
- The thread built-ins other than `thread.yield`. They stay unsupported at
  translation.
- A provider for the suspend seam. When a built-in of this design must block
  where the reference permits it, the nested turn of [PDD018] serves it. Its
  failure is the deadlock cause when the store is idle and the stack-switch
  cause when a host task is still pending.

## The Callback Export

A callback export is lifted with `canon lift async (callback $cb)`. Validation
requires the function type to be `async` and the callback to have the core type
`(func (param i32 i32 i32) (result i32))`. The translator of [PDD006] already
emits an initializer that extracts the callback from the core instance, and the
polyfill rejects it today. This design accepts it. The callback joins the
memory, the realloc, and the post-return of an instance as one more extracted
function. The lift options of the export name it.

The core function of a callback export has the flattened parameters of a
synchronous export and one `i32` result, the status word. When the flattened
parameters exceed sixteen, the export takes one pointer instead, as a
synchronous export does. The export does not return its result. It calls
`task.return`, whose parameters are the flattened result. When the flattened
result exceeds sixteen values, `task.return` takes one pointer. A callback
export has no post-return step. The reference calls `post-return` only on the
synchronous path, and the runtime never calls one for an asynchronous task.

The type projection accepts an `async` function type on an export. On an import
it stays refused with `Error::Unsupported`, because no host function of this
design can satisfy it. The public `FunctionType` gains one boolean, `async_`,
which is true when the function type carries the `async` effect. Wasmtime
exposes the same fact as `ComponentFunc::async_`. The typed conversion of
[PDD010] compares parameters and result as today and ignores the flag. A typed
handle to a callback export therefore works with no change to the host's code.

## The Task of a Host Call

A host call into a callback export is a task, created as [PDD018] creates every
task. The differences from a synchronous export are in the item that runs the
implicit thread and in when the call's future resolves.

```text
fn call_callback_export(store, export, args) -> Future<Result<Val>>:
    task = store.create_task(export.function, export.options, export.instance)
    item = TaskStart:
        cx = BoundaryContext(export.options, export.instance, task)
        flat = cx.lower(args, export.function.params)
        task.start()                     // state: started
        word = call_and_trap(export.core, flat)
        handle_status_word(task, word)
    // holds the item at the entry gate of the runtime model until the gate
    // opens, then takes the exclusive thread and queues the item
    store.scheduler.enter_implicit_thread(task, async_function = true,
                                          needs_exclusive = true, item)
    return Driver(store, task, until task.result is set)
```

The gate is the entry gate of the runtime model. A callback task needs the
exclusive thread of its instance. The callback form runs core code only between
events, and that code must not overlap another exclusive task of the same
instance. When the gate holds the task, the task is a queued item, not a
suspended thread. The call's driver, when its turn finds the task at the gate
and nothing else ready, fails with the deadlock cause. A synchronous export
ignores the gate, as today.

The arguments are lowered after the gate opens, as the reference's `start`
lowers them. A `realloc` the lowering calls is a task with one thread, as the
destructor task below is. The reference lifts `realloc` as a function and
invokes it, so a slot the realloc sets ends with its thread and the export's
core function sees zero. Wasmtime's corpus proves that rule for a host call.

The call's future is a driver. It resolves when `task.return` sets the task's
result, and it returns the lifted value. The export's core function has by then
returned a status word, or it is still on the stack below `task.return`. In the
second case the result is stored and the driver sees it in the same turn, after
the core function returns. The task does not end when the call resolves. The
status word decides what the task does next:

- Exit ends the implicit thread. If the task has not returned, the thread's exit
  is the no-result trap. Otherwise the task record leaves the store, as a
  synchronous task's record does when its call returns.
- Yield or wait leaves a callback item in the store. The item runs in the turn
  of whichever driver comes next: a later call, an instantiation, or the
  `run_concurrent` entry of [PDD018]. An error the item raises fails that
  driver, not the call that started the task. This is Wasmtime's rule for a task
  that keeps running after it returns. It is also what the explainer means by
  returning early to do cleanup afterwards.

Dropping the call's future cancels nothing, per [PDD018]. The task stays in the
store and runs in the next turn of any driver.

A task whose function type is synchronous must not block before it returns. Once
it has returned, or when its function type is `async`, it is allowed to block.
The rule is lazy, as the reference and Wasmtime state it. A synchronous task
that has to block first runs the ready threads of its own instance, and it traps
with the cannot-block cause only when none remains. In this design no such
thread can exist, so the trap follows at once. A callback task is allowed to
block from its start. The may-not-suspend flag of the instance record of
[PDD018] marks a synchronous call in progress. A host call into a synchronous
export sets it for the length of the call, as the enter intrinsic sets it for a
synchronous call between components.

The scope stack of [PDD018] sees a callback task as it sees every task. The
start item pushes the task as the current scope before it lowers the arguments.
It pops the task after the core function returns its status word. Every callback
invocation pushes the task again and pops it when the callback returns. A borrow
the host lowered in counts against the task. `task.return` traps while the count
is above zero, which is the scope-exit rule of [PDD014].

## The Status Words

The status word is the `i32` the export's core function and its callback return.
Its low four bits are the code and its high bits are a waitable set index. The
codes are exit (0), yield (1), and wait (2). A code above two traps with the
unsupported-callback-code cause. The polyfill decodes the word as the
reference's `unpack_callback_result` does.

```text
fn handle_status_word(task, word):
    code = word & 0xf
    set_index = word >> 4
    trap_if(code > 2, UnsupportedCallbackCode)
    inst = task.instance
    match code:
        EXIT:
            task.exit_implicit_thread()          // traps NoResult if task is not resolved
            inst.exclusive_thread = none
        YIELD:
            inst.exclusive_thread = none
            scheduler.push_low_priority(Callback(task, event = (NONE, 0, 0)))
        WAIT:
            set = inst.handles.get(set_index)    // traps if the entry is not a waitable set
            inst.exclusive_thread = none
            if set.has_pending_event():
                scheduler.push_high_priority(Callback(task, event = set.take_event()))
            else:
                task.implicit_thread.readiness = WaitableSet(set)
                set.num_waiting += 1

fn run_callback(task, event):
    inst = task.instance
    if inst.exclusive_thread is set:             // another exclusive task holds it
        scheduler.defer(Callback(task, event))   // runs again when the holder releases
        return
    inst.exclusive_thread = task.implicit_thread
    push_scope(task)
    word = call_and_trap(task.options.callback, [event.code, event.p1, event.p2])
    pop_scope()
    handle_status_word(task, word)
```

Three rules of the reference shape this loop:

- The exclusive thread is held while core code runs and released between events.
  A synchronous export of the same instance can therefore run while a callback
  task waits. The explainer names this as the reason the callback form allows
  more concurrency than a synchronous export.
- A yield always gives way. The callback item goes on the low-priority queue of
  [PDD018]. It runs after every other ready item, and its resumption first
  returns control to the host executor. The event it receives is the none event,
  `(0, 0, 0)`. Natively the driver wakes itself. In the browser the wake crosses
  a macrotask boundary. Both are the yield rule of [PDD018].
- A wait on a set that already holds an event does not block. The callback item
  goes on the high-priority queue with that event. A wait on a set with no event
  records the set as the thread's readiness condition. When a later turn fills
  an event in one of the set's waitables, the scheduler queues the callback item
  with that event. The set delivers events in join order. When no turn ever
  does, the call's driver goes idle and fails with the deadlock cause. That is
  Wasmtime's deadlock trap, and the corpus expects its message for a callback
  that waits on an empty set.

The event is the triple of [PDD018]: a code and two payloads. For a wait, the
payloads are the waitable's index in the handle table and its state or result.
In this design no waitable kind exists in a table, only waitable sets. No turn
ever fills an event, so a wait on an empty set always ends in the deadlock
cause. The delivery path is designed and built now. A test seam that fills an
event in a set proves it. The feature that adds the first waitable kind then
changes nothing here.

## task.return

`task.return` is lifted with a result type and options, and its core signature
is the flattened result. It resolves the current task.

```text
fn task_return(builtin, flat_args):
    task = current_task()
    trap_if(not task.instance.may_leave, CannotLeave)
    trap_if(not task.options.async, ReturnFromSynchronousTask)
    trap_if(builtin.result_type != task.function.result, ReturnMismatch)
    trap_if(builtin.string_encoding != task.options.string_encoding
            or builtin.memory is not task.options.memory, ReturnMismatch)
    cx = BoundaryContext(builtin.options, task.instance, task)
    result = cx.lift(flat_args, task.function.result)
    trap_if(task.state == resolved, ReturnedTwice)
    trap_if(task.num_borrows > 0, OutstandingBorrows)  // the scope-exit rule
    task.resolve(result)
```

The result type comparison is structural. It compares the projected type of the
built-in's result with the projected result of the task's function. Wasmtime
compares type indices, and the reference compares its types by value. The option
comparison compares the string encoding and the identity of the memory instance.
The reference's `LiftOptions.equal` compares exactly those two. Wasmtime's
comparison takes the same two plus the data model, which is linear memory in
every option the polyfill accepts. A `task.return` whose options name no memory,
when the result needs none, passes the memory comparison. The lift runs through
one boundary context built from the built-in's options, the instance, and the
task. A string or a list result therefore reads the task's memory, as [PDD018]
requires.

A `task.return` from a synchronous export traps. The reference traps on
`not task.opts.async_`. Wasmtime has no distinct trap for it, so the message is
the polyfill's own. Every other trap has Wasmtime's message.

## Backpressure

Each instance record of [PDD018] holds a backpressure counter.
`backpressure.inc` adds one and traps when the counter reaches 65536.
`backpressure.dec` subtracts one and traps when the counter falls below zero.
Both traps carry Wasmtime's message, which names an overflow in both directions.
Neither built-in reads the instance's may-leave flag. The reference exempts
them, and the corpus proves that a realloc can call them.

The counter's effect is in the entry gate. A task of an `async` function type
whose instance has a counter above zero waits at the gate until the counter
returns to zero. A synchronous export ignores the counter. With one host call at
a time, the corpus observes the gate in three steps. A synchronous export raises
the counter and returns. A host call into a callback export then waits at the
gate. The driver finds nothing else ready and fails with the deadlock cause.

## Waitable Sets from One Task

A waitable set is a handle table entry of the kind [PDD018] reserved. The five
built-ins reach the set records of the store through the current instance's
handle table. Each one but `waitable-set.new` traps when the index does not name
a waitable set. Every one of them traps when the instance's may-leave flag is
clear.

- `waitable-set.new` inserts a set record and returns its index.
- `waitable-set.wait` takes a set index and a pointer. If the set holds an
  event, the built-in takes the first event in join order. It writes the two
  payloads as `u32` values at the pointer and at the pointer plus four, in the
  built-in's memory, and returns the code. If the set holds no event, the
  built-in asks the suspend seam of [PDD018] to suspend the thread until the set
  holds an event. On a target with no provider, the nested turn runs. For a task
  that must not block, the nested turn runs only ready work of the task's own
  instance, and the built-in fails with the cannot-block cause when that work
  does not fill the set. That is the case of a start function or a synchronous
  export. For a task that is allowed to block, the built-in fails with the
  deadlock cause when the store goes idle with no event, and with the
  stack-switch cause when a host task is still pending.
- `waitable-set.poll` takes the same arguments and never blocks. It returns the
  none code and writes nothing when the set holds no event.
- `waitable-set.drop` removes the entry. It traps when the set still holds a
  waitable and when a thread is waiting on it, with the waitable causes of
  [PDD018].
- `waitable.join` takes a waitable index and a set index. A set index of zero
  removes the waitable from its set. Otherwise the waitable moves into the named
  set and leaves its previous one. The built-in traps when the first index is
  not a waitable, when the second is not a set, and when the waitable has a
  synchronous waiter.

In this design every waitable kind is reserved. `waitable.join` therefore finds
no waitable to join in the corpus, and `waitable-set.wait` in a synchronous task
always finds an empty set. The corpus proves that case through a start function
that waits, which fails instantiation with the cannot-block message.

## Context Slots

The two context slots live on the thread record, as [PDD018] moved them. A
thread starts with both slots at zero. `context.get` reads and `context.set`
writes the slot of the current thread. The slots persist across every resumption
of a callback task, because the implicit thread persists until the callback
exits. They end with the thread. Neither built-in reads the may-leave flag, and
the corpus proves that a realloc can call them.

Two consequences follow from the task lifecycle above. A realloc the host's
argument lowering calls runs on its own thread, so it sees zeros and what it
sets does not reach the export. The same holds for a realloc that lowers a host
function's result into the guest. A destructor runs on its own thread, so it
sees zeros and what it sets does not reach the thread that dropped the handle.

## thread.yield

`thread.yield` gives way and returns zero. The reference treats a yield as a
point where any other ready thread can run, and Wasmtime switches to a ready
thread of the instance when one exists. The built-in asks the suspend seam of
[PDD018] for one chance to be given back control, which is what a yield waits
for and no more. On a target with no provider, the nested turn runs the ready
work once and the built-in returns. A task that must not block gives way only to
ready work of its own instance, and with none the yield is a no-op, as Wasmtime
runs it.

The built-in has no rule of its own that fails, which is what the reference
states: `canon_thread_yield` has the may-leave trap and otherwise always answers
zero. This design keeps that. The built-in returns zero whenever it returns,
whatever the nested turn found, and it traps when the may-leave flag is clear.
Two things stop it returning, and neither is a rule of the yield's. A failure of
the work the nested turn ran is that work's failure, and the built-in hands it
on. A failure of the suspension itself is the seam's, and it ends the call the
giving thread is inside rather than answering the yield.

From inside a guest frame with no stack switch, the polyfill cannot hand control
to the host executor, so a callback task that wants the executor to run returns
the yield status word instead.

## The Destructor Task

`resource.drop` on an owned handle runs the resource's destructor. The reference
lifts the destructor as a synchronous function of one `u32` parameter and lowers
a call to it. Every destructor run is therefore a task with one thread, with the
host release of [PDD017] and a guest drop alike. The task is pushed as the
current scope before the destructor runs and popped after it returns. Its thread
starts with zero context slots, and its slots end with it. A destructor that
drops another resource nests a second destructor task the same way. Wasmtime's
corpus proves this for destructors, and the direct call the polyfill makes today
fails those directives.

## Translation

The translator accepts what this design builds. It refuses the rest with
`Error::Unsupported`, the rule [PDD006] states for every option and built-in the
polyfill does not implement:

- Accepted: the callback initializer, `canon lift async` with a callback, an
  `async` function type, and the trampolines for `task.return`,
  `backpressure.inc`, `backpressure.dec`, `waitable-set.new`,
  `waitable-set.wait`, `waitable-set.poll`, `waitable-set.drop`,
  `waitable.join`, and `thread.yield`.
- Refused: `canon lift async` without a callback, an `async` function type on an
  import, `canon lower async`, the prepare-and-start adapter trampolines, and
  `task.cancel`. Also refused: the subtask, stream, future, and error-context
  built-ins, every thread built-in other than `thread.yield`, and the table
  initializer that `thread.new-indirect` needs.

## Error Model Growth

`wcmp::Error` gains one variant for the task built-ins, `Task`, with a
structured cause. Each message is the trap Wasmtime prints where one exists, so
the corpus matches it by substring:

- No result: the implicit thread exited and the task had not returned.
  Wasmtime's `NoAsyncResult`.
- Returned twice: `task.return` ran on a resolved task. Wasmtime's
  `TaskCancelOrReturnTwice`.
- Return mismatch: the result type or the options of `task.return` differ from
  the lift. Wasmtime's `TaskReturnInvalid`.
- Return from a synchronous task: `task.return` ran in a task whose lift is not
  `async`. The message is the polyfill's own.
- Unsupported callback code: a status word whose code is above two. Wasmtime's
  `UnsupportedCallbackCode`.
- Backpressure overflow: the counter left its range. Wasmtime's
  `BackpressureOverflow`.
- Cannot leave: a built-in that reads the may-leave flag ran while the flag was
  clear, from a realloc or a post-return. Wasmtime's `CannotLeaveComponent`.

The scheduler causes of [PDD018] apply as that document states. A driver that
goes idle with a callback task at the gate or waiting on an empty set fails with
the deadlock cause. A synchronous task that waits on an empty set fails with the
cannot-block cause. A callback task whose core function waits on an empty set,
on a target with no suspend provider, runs a nested turn. It fails with the
deadlock cause when the store goes idle and with the stack-switch cause when a
host task is still pending. No host task exists in this design, so the deadlock
cause is the outcome. The waitable causes of [PDD018] apply to
`waitable-set.drop` and `waitable.join`. The outstanding-borrows cause of
[PDD014] applies to `task.return`.

## Target Differences

Nothing in this document differs between the native target and the browser
beyond the four differences [PDD018] states. The one this feature reaches is the
wake after a yield, when a callback returns the yield word. It is a self-wake
natively and a macrotask hop in the browser.

## The Corpus

The `async` directories of both corpora are vendored, and every failing
directive is listed as a deferred feature. This feature removes the lines its
scope owns and leaves the rest with their reasons. The harness registers one
more host item, because one directive imports it. It is a `wasmtime` instance
whose `gc` function does nothing, as Wasmtime's wast runner registers it.

The files this design owns, in the Component Model corpus:

- `dont-block-start.wast`, its first directive. A core start function waits on
  an empty set and instantiation fails with the cannot-block message. Its second
  directive calls a callback export from a sibling component through a
  synchronous lower and stays deferred.
- `validate-no-async-abi-for-sync-type.wast`. Its three directives pass already.
  They prove that validation refuses the `async` option on a synchronous
  function type.

In the Wasmtime corpus:

- `lift.wast`. A component with a callback export instantiates.
- `drop-deadlock.wast`. A synchronous export raises backpressure, and a host
  call into a callback export fails with the deadlock message.
- `backpressure-overflow.wast`. The counter traps at both ends of its range and
  survives 65535 increments.
- `task-return-traps.wast`, its first directive. A callback export exits without
  `task.return` and the call fails with the no-result message. Its second
  directive needs a thread built-in. Its other five directives lift without a
  callback and stay deferred.
- `sync-call-context.wast` and `sync-call-context-slots.wast`. Synchronous calls
  between components keep their context slots when the callee raises and lowers
  backpressure inside the call.
- `context-in-resource-drop.wast`. Every directive. A destructor sees zero slots
  and does not disturb the dropper's, alone, nested, across a composition, and
  after a host call.
- `task-builtins.wast`, its single-instance directives. Those are the six
  components that each define one owned built-in, and the may-leave exemption of
  backpressure and the context slots. They are also the two directives whose
  realloc calls those built-ins, once before the export runs and once around a
  call to a synchronous host item. Its subtask components, its `async`
  permutations across two components, and its stream and future case stay
  deferred.

The files that stay deferred, with the reason:

- A call into an asynchronous export from a sibling component: the second
  directive of `dont-block-start.wast`, `deadlock.wast`,
  `drop-waitable-set.wast`, `wait-forever.wast`, `wait-forever2.wast`,
  `callback-yield-then-exit.wast`, `many-params-with-retptr.wast`,
  `subtask-wait.wast`, `fused.wast`, `reenter-during-yield.wast`, and the
  `async` permutation of `context-in-compositions.wast`.
- Future or stream built-ins: `empty-wait.wast`, `wait-during-callback.wast`,
  and every stream and future file.
- The stackful lift: five directives of `task-return-traps.wast` and
  `stackful.wast`.
- Thread built-ins: the second directive of `task-return-traps.wast`,
  `self-switch-traps.wast`, `join-during-sync-read.wast`, and the
  `during-sync-*.wast` group.
- Cancellation and error contexts: every file that names them.
- An asynchronous host item: `lower.wast` and every file that imports one of the
  four asynchronous spectest items.

## User Stories

A developer calls an `async` export of a component from a browser page.

> The developer awaits the call as before. The export returns the yield word
> twice while it works, and the page handles input between the turns. The export
> calls `task.return` and the promise resolves with the result. The callback
> runs once more and exits in the next turn.

A developer reads an export's type before calling it.

> The developer sees `async_` set on the function type. The developer follows
> the call with `run_concurrent`, so that the task's cleanup after its return
> runs before the page moves on.

A guest author returns early and cleans up afterwards.

> The export calls `task.return` with its result and then returns the yield
> word. The caller has its value. In the next turn the callback frees the
> buffers the call used and returns exit.

A guest author exerts backpressure.

> A synchronous export raises the counter while a buffer is full. A host call
> into an `async` export waits at the gate. With nothing else to run, the call
> fails with the deadlock error, and the host reads in it that the instance
> refused entry.

A contributor adds the first waitable kind.

> The contributor fills an event in a waitable that a set holds. The scheduler
> queues the waiting task's callback with that event and the contributor changes
> nothing in this design.

## Test Cases

A callback export returns at once. A host calls an export that calls
`task.return` and returns the exit word in its first call. The call resolves
with the lifted result, the task is resolved, and the exclusive thread of the
instance is released. `lift.wast` instantiates, and a repository test proves the
result on a component written by hand.

A callback export yields and is resumed. An export calls `task.return`, returns
the yield word, and its callback receives the none event in a later turn and
returns exit. The call resolves at `task.return`. The callback runs after every
other ready item and after the driver returned control to the executor once. Its
context slots hold what the export set. A repository test proves it, because no
corpus directive reaches the yield word from the host.

A callback export waits on an empty set. An export returns the wait word with a
set that holds no event. The call fails with the deadlock cause, and the message
is Wasmtime's. `drop-deadlock.wast` proves the gate variant of the same failure,
where backpressure holds the task and the call fails with the same message. A
repository test proves the wait variant.

A wait is satisfied by an event. An export returns the wait word. A test seam
fills an event in a waitable of the set. The callback receives that event with
the waitable's index and payload. A second wait on a set that already holds an
event runs in the next turn without an idle. A repository test proves both.

The status word is decoded as the reference decodes it. A code above two fails
with the unsupported-callback-code cause. The set index is the high bits of the
word. A repository test proves it.

`task.return` traps as the reference states. Exit without a return fails with
the no-result cause, on the first directive of `task-return-traps.wast`. A
mismatched result type, a mismatched string encoding, and a mismatched memory
each fail with the return-mismatch cause. A second return fails with the
returned-twice cause. A return from a synchronous export fails with its own
cause. A return with an outstanding borrow fails with the outstanding-borrows
cause. A string result lifts through the task's memory. Repository tests prove
the cases the corpus does not name.

Backpressure gates and traps. `backpressure-overflow.wast` passes whole.
`drop-deadlock.wast` passes whole. A repository test queues two tasks through
the store's records. It proves that the second waits at the gate while the
counter is above zero and starts when the counter returns to zero.

A synchronous task cannot block. The first directive of `dont-block-start.wast`
fails instantiation with the cannot-block message. A repository test proves that
`waitable-set.wait` from a synchronous export on a set that holds an event
returns that event without blocking.

`thread.yield` gives way. A repository test proves that a yield in a callback
task's core function runs a queued item of another task before it returns, and
that a yield in a synchronous export with no ready work of its own instance
returns zero at once. The built-in never fails of a rule of its own. Repository
tests prove that every yield of a guest that gives way and then returns answers
zero, whether or not a caller's frame is below it.

Waitable sets from one task behave as the reference states. `waitable-set.poll`
on an empty set returns the none code. `waitable-set.drop` on a set that holds a
waitable, and on a set a thread waits on, fails with the waitable causes.
`waitable.join` with a set index of zero removes the waitable from its set. Each
built-in fails with the cannot-leave cause when a realloc calls it. Repository
tests prove them, and the components of `task-builtins.wast` that define each
built-in instantiate.

Context slots belong to the thread. `sync-call-context.wast`,
`sync-call-context-slots.wast`, and the single-instance directives of
`task-builtins.wast` pass. A slot set by a realloc during the host's argument
lowering, or during the lowering of a host function's result, is not visible to
the export or to the task that made the call.

A destructor runs as a task. `context-in-resource-drop.wast` passes whole, with
the `wasmtime` `gc` host item registered. A destructor sees zero context slots,
a slot it sets does not reach the dropper, and a nested destructor gets its own
slots.

The function type states `async`. A host reads `async_` as true on a callback
export's type and false on a synchronous export's type. A typed handle to a
callback export is acquired and called with no change to the host's code.

A late trap fails the next driver. An export calls `task.return`, returns the
yield word, and its callback traps. The call that started the task resolves with
its result. The next driver of the store fails with the trap. A repository test
proves it.

The corpus lines are removed. The expected-failure list loses every line the
owned files and directives above account for, and no other line. The progress
summary shows the two `async` rows with the new counts, the same on both
targets.

Every facet above holds on both targets. The native run and the browser run
report the same pass and failure results for every named file and every
repository test.

## References

- [PDD006], component parsing, whose translator emits the callback initializer.
- [PDD010], the typed call surface.
- [PDD014], borrow lifetime tracking, whose scope-exit rule `task.return`
  applies.
- [PDD016], the conformance suite.
- [PDD017], resource disposal. This document states the task a destructor runs
  in.
- [PDD018], the concurrency runtime model this feature builds on.
- [Concurrency], the Concurrency explainer, and its sections on the [stackless
  async export][Stackless], [thread-local storage][TLS],
  [backpressure][Backpressure], [returning][Returning], and [the start
  function][Start].
- [CanonicalABI – canon lift], the callback loop of the reference, and the
  sections on [`task.return`][CanonicalABI – task.return],
  [`backpressure.{inc,dec}`][CanonicalABI – backpressure],
  [`waitable-set.wait`][CanonicalABI – waitable-set.wait],
  [`waitable-set.drop`][CanonicalABI – waitable-set.drop],
  [`waitable.join`][CanonicalABI – waitable.join], and
  [`thread.yield`][CanonicalABI – thread.yield].
- [`definitions.py`], the executable reference for `canon_lift`, `CallbackCode`,
  `canon_task_return`, the backpressure, waitable set, context, and yield
  built-ins, and `canon_resource_drop`.
- [Explainer – canonopt], the validation rules for the `async` and `callback`
  options.
- [Wasmtime], the reference implementation, its [callback handling], its [trap
  messages], and [`ComponentFunc::async_`].
- The [Component Model test corpus] and the [Wasmtime component tests].

[PDD006]: ./PDD006%20Component%20Parsing.md
[PDD010]: ./PDD010%20Pre-wasip3%20Public%20API.md
[PDD014]: ./PDD014%20Borrow%20Lifetime%20Tracking.md
[PDD016]: ./PDD016%20Conformance%20Suite.md
[PDD017]: ./PDD017%20Resource%20Disposal.md
[PDD018]: ./PDD018%20Concurrency%20Runtime%20Model.md
[Concurrency]:
  https://github.com/WebAssembly/component-model/blob/main/design/mvp/Concurrency.md
[Stackless]:
  https://github.com/WebAssembly/component-model/blob/main/design/mvp/Concurrency.md#stackless-async-exports
[TLS]:
  https://github.com/WebAssembly/component-model/blob/main/design/mvp/Concurrency.md#thread-local-storage
[Backpressure]:
  https://github.com/WebAssembly/component-model/blob/main/design/mvp/Concurrency.md#backpressure
[Returning]:
  https://github.com/WebAssembly/component-model/blob/main/design/mvp/Concurrency.md#returning
[Start]:
  https://github.com/WebAssembly/component-model/blob/main/design/mvp/Concurrency.md#interaction-with-the-start-function
[CanonicalABI – canon lift]:
  https://github.com/WebAssembly/component-model/blob/main/design/mvp/CanonicalABI.md#canon-lift
[CanonicalABI – task.return]:
  https://github.com/WebAssembly/component-model/blob/main/design/mvp/CanonicalABI.md#-canon-taskreturn
[CanonicalABI – backpressure]:
  https://github.com/WebAssembly/component-model/blob/main/design/mvp/CanonicalABI.md#-canon-backpressureincdec
[CanonicalABI – waitable-set.wait]:
  https://github.com/WebAssembly/component-model/blob/main/design/mvp/CanonicalABI.md#-canon-waitable-setwait
[CanonicalABI – waitable-set.drop]:
  https://github.com/WebAssembly/component-model/blob/main/design/mvp/CanonicalABI.md#-canon-waitable-setdrop
[CanonicalABI – waitable.join]:
  https://github.com/WebAssembly/component-model/blob/main/design/mvp/CanonicalABI.md#-canon-waitablejoin
[CanonicalABI – thread.yield]:
  https://github.com/WebAssembly/component-model/blob/main/design/mvp/CanonicalABI.md#-canon-threadyield
[`definitions.py`]:
  https://github.com/WebAssembly/component-model/blob/main/design/mvp/canonical-abi/definitions.py
[Explainer – canonopt]:
  https://github.com/WebAssembly/component-model/blob/main/design/mvp/Explainer.md#canonical-definitions
[Wasmtime]: https://github.com/bytecodealliance/wasmtime
[callback handling]:
  https://github.com/bytecodealliance/wasmtime/blob/v49.0.0-rc.1/crates/wasmtime/src/runtime/component/concurrent.rs
[trap messages]:
  https://github.com/bytecodealliance/wasmtime/blob/v49.0.0-rc.1/crates/environ/src/trap_encoding.rs
[`ComponentFunc::async_`]:
  https://docs.wasmtime.dev/api/wasmtime/component/types/struct.ComponentFunc.html#method.async_
[Component Model test corpus]:
  https://github.com/WebAssembly/component-model/tree/main/test/async
[Wasmtime component tests]:
  https://github.com/bytecodealliance/wasmtime/tree/main/tests/misc_testsuite/component-model/async
