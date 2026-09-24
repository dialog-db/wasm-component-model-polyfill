# Stack Switching, Stackful Exports, and Threads

[PDD018] gave the scheduler one suspend capability and left it empty on both
targets. [PDD019], [PDD020], and [PDD021] built the callback export, the
subtask, and streams and futures on the fallback, the nested turn. This document
fills the capability. It also designs the two features that need it: the
stackful form of `canon lift async`, and the eight thread built-ins other than
`thread.yield`.

A stack switch is the act of setting aside a running guest stack and resuming it
later. Core WebAssembly as the browsers ship it has no stack switch. A stack
switch comes from an engine API, from the stack-switching proposal, or from a
rewrite of the guest. This design uses the first two and never rewrites a guest.
It never uses the internals of an engine. It uses only a JavaScript API that
every current browser ships, and WebAssembly instructions that an engine
implements or does not.

The design follows the [Concurrency explainer][Concurrency] and the Python
reference in [`definitions.py`] at the commit the conformance corpus of [PDD016]
is vendored from. Where the reference leaves a choice to the host, the design
makes the choice [Wasmtime] makes at `v49.0.0-rc.1`. Where it departs from
either, it states the reason.

Five terms recur:

- A provider is a mechanism that fills the suspend capability.
- The switch module is a core module the polyfill generates for a store. Both
  providers use it.
- A thread entry is a guest function that starts a thread: a task's core
  function, a callback, or a thread's start function.
- A blocking built-in is a built-in that the reference lets wait inside a guest
  call.
- A nested start is a thread that a trampoline starts on a new stack from inside
  itself.

## Goals

- The suspend capability of [PDD018] has one contract. Each guest thread runs on
  a stack of its own and suspends only inside a blocking built-in. A suspended
  thread resumes later with the result of that built-in.
- Two providers fill the contract. The stack-switching provider uses the
  instructions of the stack-switching proposal on any engine that implements
  them. The JSPI provider uses JavaScript Promise Integration in the browser.
- The polyfill selects a provider once per `Engine`, in a fixed order, from
  probes that do not depend on the engine. Where no provider exists, the nested
  turn of [PDD020] stays the fallback.
- A host can turn the provider off through `EngineConfig`, and can read which
  provider an `Engine` selected.
- A blocking built-in follows the reference's thread model under a provider. It
  sets up a wait with a readiness condition that only reads the store, and it
  completes when the scheduler resumes the thread.
- A thread that a trampoline starts runs as a nested start above that
  trampoline, as the reference runs it.
- The stackful lift and all eight thread built-ins translate on every target
  behind the existing `EngineConfig` gates. They fail only at a block the target
  cannot serve, with the stack-switch cause.
- A block that only a frame below it can release fails with the stack-switch
  cause and not the deadlock cause.
- The browser backend enters a host function at any depth, as a native engine
  does. The re-entrant host-call refusal is removed.
- The conformance harness measures each target with its provider and without
  one.

## Non-goals

- A rewrite of guest modules, such as Binaryen's Asyncify. The costs are stated
  under Why No Guest Rewrite.
- The use of any engine's internal fibers. Wasmtime's `Func::call_async` and the
  pause of wasm3 serve one nested pause at a time. The contract needs threads
  that suspend and resume independently of each other.
- An implementation of any Wasm Core proposal. The switch module uses the
  stack-switching proposal where an engine implements it. The polyfill
  implements no instruction.
- The shared-everything thread built-ins `thread.spawn-ref`,
  `thread.spawn-indirect`, and `thread.available-parallelism`.
- Cancellation, including a cancellable form of any thread built-in. The
  cancelled event is not delivered.
- The rules that decide which trap poisons an instance.
- A cap on the number of live threads. The limits of the engine apply.

## Facts This Design Rests On

Each fact below was read from the cited source or measured by a throwaway
prototype on an x86_64 Linux host.

- JSPI ships in Chrome and Edge 137, Firefox 153, and Safari 27.0 ([caniuse],
  [Safari 27.0]).
- A call through `WebAssembly.promising` runs the guest synchronously up to the
  first call of a suspending import. Chromium 147 measured this.
- A suspending import made with `WebAssembly.Suspending` always suspends. It
  suspends when the JavaScript function returns a plain value or a resolved
  promise, too. Chromium 147 measured this. The proposal's overview states the
  opposite, so the design assumes the measured behavior.
- A suspension traps if a frame that is not WebAssembly lies between the
  promising entry and the suspending import ([JSPI]).
- A promising call made from inside a plain import of another promising stack
  starts an independent stack. Either stack resumes first. Chromium 147 measured
  this.
- The browser backend passes one instance's exported function to another
  instance as the function object itself. A call between two core instances is
  therefore a call from WebAssembly to WebAssembly. Source:
  `create_imports_object` in the vendored `js_wasm_runtime_layer`.
- Wasmtime 49 implements the stack-switching proposal on x86_64 Linux only
  ([Wasmtime stack switching]). The feature is off by default and turns off
  compiler inlining.
- Under Wasmtime 49 with the feature on, a host function that runs inside one
  continuation can start a second continuation and get control back when the
  second suspends. A suspension crosses frames of two other instances. Two
  suspended continuations resume in either order. The prototype measured all
  three.
- wasm3 implements the stack-switching proposal as an interpreter, on every
  platform it builds for ([wasm3 stack switching]).
- An engine keeps state that assumes calls nest. Wasmtime saves and restores its
  activation list on each switch of its own fibers ([Wasmtime activations]).
  wasm3 saves its native stack limit on entry and restores it on exit ([wasm3
  stack limit]). A stack switch that an engine does not perform itself breaks
  that state. That is why the providers use only an engine API or the engine's
  own instructions.
- wit-bindgen generates the callback form for every language it supports. Its
  core ABI marks the stackful export as not supported ([wit-bindgen ABI]). Its C
  generator exposes the thread built-ins to C code ([wit-bindgen C]). A guest
  that runs its own threads, such as C with pthreads, needs a stack switch.

## The Suspend Capability

[PDD018] named one suspend capability and designed no provider. This document
states the contract a provider must meet. A provider must:

- Start a thread entry on a stack of its own, from the scheduler or from inside
  a trampoline.
- Suspend the running thread when a blocking built-in is not ready. Only
  WebAssembly frames lie between the start of that stack and the point of
  suspension.
- Resume a suspended thread later, with the result of the built-in it suspended
  in.
- Keep any number of threads suspended at once, and resume them in any order.
- Report to the caller whether a thread entry finished or suspended, at the
  moment it does so.
- Drop a suspended thread without resuming it when its store drops. No
  destructor runs, as [PDD018] states for the store.

A mechanism that cannot meet all six is not a provider. The polyfill never fills
the capability with less. Where no provider exists, the nested turn of [PDD020]
serves each blocking built-in under its five rules, unchanged.

A resumption can complete at once or later. The stack-switching provider resumes
a thread synchronously. The JSPI provider resumes a thread on a microtask, which
runs before the browser returns to its event loop. The scheduler treats both the
same way: a turn that resumes a thread waits until the thread suspends again or
finishes, and runs nothing else in between.

## The Switch Module

The polyfill generates one small core module for each store, the switch module.
It builds the module's bytes in memory, so the switch module is never fetched.
It stands between the scheduler and the guests. It has two parts, and both
providers use both:

- A shim for each blocking built-in. A guest imports the shim in place of the
  host trampoline. The shim calls a host function that tries the built-in and
  returns at once. If the built-in is ready, the shim returns its result. If
  not, the shim suspends the thread, and it completes the built-in after the
  thread resumes.
- An entry wrapper for each thread entry. The wrapper calls the entry and hands
  the entry's results to a host function before it returns. The scheduler reads
  those results from the host side at once, whether or not the entry suspended
  on the way.

The shim exists for two reasons. A suspension must have only WebAssembly frames
between it and the start of the stack, and a host trampoline is not WebAssembly.
And a suspending import always suspends, so the shim must call it only when the
built-in is not ready.

```text
shim for a blocking built-in B:
    loop:
        status = host.try_B(args)        // returns at once, never blocks
        if status is ready:
            return host.finish_B()       // the result the guest reads
        suspend                          // the provider's own form of suspend

entry wrapper for a thread entry E:
    results = E(args)
    host.finished(thread, results)       // the scheduler reads these at once
```

The providers differ only in the form of `suspend`, and in how a thread starts
and resumes.

```text
the stack of one thread under a provider

    scheduler (host)            calls the provider's start or resume
    ─────────────── the start of the thread's own stack ───────────────
    entry wrapper (Wasm)
    guest core function (Wasm)
    fused adapter (Wasm)        a call into another component
    callee core function (Wasm)
    shim (Wasm)                 suspends here
```

A host trampoline that returns before the shim suspends, such as `try_B`, is not
on the stack at the point of suspension. The rule of the first bullet holds.

## The Two Providers

### The Stack-Switching Provider

The stack-switching provider fills the capability with the instructions of the
stack-switching proposal: `cont.new`, `resume`, and `suspend`. The switch module
defines one control tag and one table of continuations. It exports a start and a
resume function to the scheduler.

```text
switch module, stack-switching form:
    tag $block
    table $threads (ref null cont)

    export start(thread, entry, args) -> status:
        return run(thread, cont.new(entry wrapper of entry))
    export resume(thread) -> status:
        return run(thread, $threads[thread])

    run(thread, c):
        resume c on $block -> parked
        return finished
      parked(k):
        $threads[thread] = k
        return suspended

    suspend in a shim:  suspend $block
```

A resumption runs synchronously and returns when the thread suspends again or
finishes. The provider works on any engine that implements the proposal. Today
those are Wasmtime 49 on x86_64 Linux and wasm3. No browser ships the proposal.

### The JSPI Provider

The JSPI provider fills the capability with the JavaScript API. A thread starts
through `WebAssembly.promising` over its entry wrapper. The `suspend` of a shim
is a call to an import made with `WebAssembly.Suspending`. The JavaScript
function behind that import returns a promise the scheduler holds. To resume the
thread, the scheduler resolves the promise with the built-in's result.

A promising call returns a promise and never the entry's results. The entry
wrapper is therefore how the scheduler learns at once that an entry finished.
The wrapper must be WebAssembly. A JavaScript wrapper puts a frame that is not
WebAssembly between the start of the stack and the suspension.

A resumption under JSPI runs on a microtask, never inside the call that asks for
it. This matters in one place. The reference sometimes resumes a suspended
thread from inside a trampoline and continues in that trampoline once the thread
stops. Two cases exist:

- A synchronous call of a sync-typed function runs the ready threads of its own
  instance until its task resolves ([`definitions.py`], `canon_lift`).
- A thread that a nested start began switches to a suspended thread before it
  first suspends.

The JSPI provider cannot resume a suspended stack from inside a synchronous
frame. In both cases it suspends the thread the trampoline runs in as well. The
scheduler then runs only the threads the reference runs from that point, in the
reference's order. It runs no other item and polls no host task. It then resumes
the trampoline's thread with the outcome. A guest cannot observe the difference,
because no other guest code runs in the interval.

### No Provider

Without a provider, a thread runs on the real stack. The nested turn of [PDD020]
serves each blocking built-in, with its five rules and its one budget. The
stackful lift and the thread built-ins run there too. A thread entry that never
blocks runs to its end. A thread that blocks succeeds when the work that
releases it can run above it. It fails with the stack-switch cause when that
work lies on a frame below it.

## Selecting a Provider

The polyfill selects the provider once, when an `Engine` is constructed, and
keeps the answer for the life of the engine. The order is:

1. The stack-switching provider, if the switch probe passes.
2. The JSPI provider, if `WebAssembly.Suspending` and `WebAssembly.promising`
   exist as functions. The probe runs in the browser only.
3. No provider.

The switch probe compiles and instantiates a module of about 130 bytes. The
polyfill carries the module as a constant in its own binary, so the probe never
fetches anything. The probe starts one thread, suspends it, resumes it, and
makes sure that it finished. An engine that rejects the module, or that runs it
with any other outcome, fails the probe. The probe proves that the feature
works, not only that the engine validates it. The two probes are small and
synchronous, so `Engine::new` stays synchronous.

The stack-switching provider comes first because it resumes a thread
synchronously. Its scheduling order then matches the native order with no
microtask between two items. Today no engine offers both providers, so the order
decides nothing yet.

The host surface grows by two items, spelled in the style of the existing
`EngineConfig` setters:

- A setting on `EngineConfig` that turns the provider off. The engine then uses
  nested turns whatever the probes find. A test lane uses it to measure the
  fallback on a target that has a provider. A host uses it to choose the
  synchronous order of nested turns, or to avoid a provider that fails on one
  engine version.
- A query on `Engine` that answers which provider it selected: the
  stack-switching provider, the JSPI provider, or none. A host uses it to
  explain a stack-switch failure to a person. The harness uses it to select the
  expected-failure lists.

Wasmtime has no counterpart, because its fibers always exist. The names are the
polyfill's own.

## Blocking Under a Provider

A blocking built-in under a provider follows the reference's thread model
([`definitions.py`], `Thread.wait_until`). It has two parts:

- The try part runs in the host trampoline the shim calls. It sets up the wait:
  it records the thread's readiness condition in the store and returns. If the
  condition already holds, the built-in is ready and the shim returns at once.
- The finish part runs when the scheduler resumes the thread. It computes the
  built-in's result and writes what the built-in writes to guest memory, such as
  the event of `waitable-set.wait`. Then the guest continues.

A readiness condition only reads the store. It changes nothing. It never polls a
host future and never runs guest code. The reference's `ready_func` has the same
property. The scheduler evaluates the conditions between items, and it resumes a
waiting thread in the order [PDD018] fixes for threads that became ready
together.

A synchronous lower of a host `async` function parks its future in the store as
a host task, in every case. Turns poll it with the driver's waker, as they poll
every host task. The lowering of its result makes the waiting thread ready. With
this rule the store always knows about a pending host future. No provider has to
tell the store about a future that a suspended frame holds.

A provider never runs turns inside a suspension, and no readiness condition runs
guest code. A suspension that a provider serves and a nested turn therefore
never meet. The provider stays in the store for the whole of a suspension, and
nothing takes it out.

A task that must not block keeps the rule of the reference. A task must not
block when it is a sync-typed call, a start function, or a resource destructor.
Its thread never lets the store run other work. When it blocks, the try part
runs the ready threads of the task's own instance from inside itself, as
`canon_lift` runs them ([`definitions.py`]). A queued item of that instance runs
as the nested turn of [PDD020], limited to the instance. A suspended thread of
that instance resumes through the provider, from inside the try part. Under
JSPI, that resumption follows the rule The JSPI Provider states. The block fails
with the cannot-block cause when no thread of the instance is ready. Wasmtime
runs the same rule through `switch_or_trap_if_may_not_suspend` ([Wasmtime
may-not-suspend]). This revises the statement of [PDD020] that such a task never
reaches the provider. It reaches the provider, but only to resume the threads of
its own instance.

## Starting a Thread

A thread starts in one of three ways:

- The scheduler starts it from a turn: a task that a host call or a driver
  began, a thread a gate released, or a thread `thread.resume-later` made ready.
- A trampoline starts it as a nested start. This covers the start intrinsics of
  [PDD020] for an async-typed callee, through a synchronous or an asynchronous
  lower, and a switch to a new thread inside a thread built-in.
- A fused adapter calls a sync-typed callee directly, on the caller's stack, as
  today. The callee's task must not block, so its thread never suspends.

A nested start runs the new thread on a stack of its own, above the trampoline
that started it. The trampoline calls the provider's start. When the new thread
suspends or finishes, the start returns to the trampoline, and the trampoline
continues. For an asynchronous lower, it returns `STARTED` or `RETURNED` to the
caller. The reference does this: `canon_lift` calls `thread.resume` from inside
the caller's `canon_lower`, and `resume` runs the new continuation from that
frame ([`definitions.py`]).

```text
async lower of a stackful callee, under a provider

    caller's stack                          callee's stack
    ──────────────                          ──────────────
    caller core function
    fused adapter
    async-start intrinsic (host)  ──start──▶  entry wrapper
                                            callee core function
                                            shim: not ready, suspend
    intrinsic continues  ◀───────suspended──
    returns STARTED to the caller
```

The trampoline's host frame stays on the caller's stack, below the start of the
callee's stack. No frame that is not WebAssembly lies between the callee's
suspension and the start of its stack. The prototype measured this shape under
both providers.

This revises the rule of [PDD020] that a suspend provider suspends the caller
and lets the driver run the callee's start item. The callee starts at once, from
inside the trampoline, with or without a provider.

## The Stackful Lift

A stackful export is an `async` lift without the `callback` option. Its core
function runs as the task's implicit thread. It returns nothing, delivers its
result through `task.return`, and blocks inside built-ins as it needs. The rules
are those of the reference ([`definitions.py`], `canon_lift`):

- The task passes the entry gate of [PDD018]. A stackful task does not need the
  exclusive thread of its instance, because only a sync lift or a callback lift
  needs it.
- The core function receives the flat parameters under the limit of the
  asynchronous lift. A stackful lift of a sync-typed function is invalid, as the
  translator already states.
- When the core function returns, the implicit thread ends. If the task has no
  other thread and has not resolved, the call fails with Wasmtime's message:
  "async-lifted export failed to produce a result".
- `task.return` resolves the task, as for a callback export. Work the task does
  after it resolves stays in the store, as [PDD019] states.

The implicit thread starts through the provider's start when a provider exists,
and as a direct call otherwise. A stackful export that never blocks behaves the
same on every target.

## The Thread Built-ins

The eight built-ins act on the thread table of the current instance. Each one
first traps when the may-leave flag of the instance is clear, as the reference
states. The behavior of each follows the reference:

| Built-in                      | What it does                                                                               | Needs a stack switch |
| ----------------------------- | ------------------------------------------------------------------------------------------ | -------------------- |
| `thread.index`                | Returns the current thread's index in the instance's thread table.                         | No                   |
| `thread.new-indirect`         | Creates a suspended thread whose start function comes from a table, and returns its index. | No                   |
| `thread.resume-later`         | Marks a suspended thread ready. It runs in a later turn.                                   | No                   |
| `thread.suspend`              | Suspends the current thread until another thread resumes it.                               | Yes                  |
| `thread.suspend-then-resume`  | Suspends the current thread and switches to a suspended thread.                            | Yes                  |
| `thread.yield-then-resume`    | Makes the current thread ready and switches to a suspended thread.                         | Yes                  |
| `thread.suspend-then-promote` | Switches to another thread if it is ready, and otherwise suspends the current thread.      | Yes                  |
| `thread.yield-then-promote`   | Switches to another thread if it is ready, and otherwise yields.                           | Yes                  |

A switch is a suspension that names the thread to run next. The frame that
resumed the current thread runs the named thread before anything else, which is
the reference's `Thread.resume` loop. The switch slot of [PDD018] holds that
thread when the frame is a turn of the scheduler.

`thread.new-indirect` reads its start function from the table that the
translator's table initializer already extracts. The start function takes one
`i32`, or one `i64` in a 64-bit memory, and returns nothing. The reference
admits both. Wasmtime 49 admits only the `i32` form, with the message "start
function does not match expected type (currently only `(i32) -> ()` is
supported)". The polyfill admits both, because it runs 64-bit memories and the
reference states the rule. This is a departure from Wasmtime. A table entry that
is empty fails with Wasmtime's message for an uninitialized start function. A
start function of another type fails with the type message.

A switch or a resume that names a thread which is not suspended fails with
Wasmtime's message: "cannot resume thread which is not suspended". A promote
that names the current thread traps, as the reference states.

Without a provider, the five built-ins that suspend take the nested turn. A
suspension there waits until a nested turn runs the work that resumes the
thread. A switch to a thread that never ran starts that thread as a nested start
on the real stack, above the trampoline. A switch to a thread that is suspended
below the current frame cannot run, and fails with the stack-switch cause.

## The Cause of a Failed Block

When a block cannot progress and its condition is unmet, the seam reports the
first cause that holds:

1. The cannot-block cause, if a task that must not block is in progress. The
   reference forbids that block on every target.
2. The stack-switch cause, if a nested start lies between the blocked thread and
   the base of the real stack. A provider returns control below that point, so
   only the target's capability is missing.
3. The stack-switch cause, if a host task is pending.
4. The deadlock cause in every other case. Then no frame below can move, and
   nothing in the store can meet the condition.

The second rule is new. It covers a callee that runs above its caller on the
real stack and waits for work only that caller does. The store marks each nested
start on its stack of current scopes, so the seam reads the rule from state the
store keeps. Wasmtime runs such a callee on a fiber of its own, so the same
shapes do not fail there. Under a provider they do not fail here either.

Under a provider, a thread that suspends does not fail. A driver whose store
goes idle while a thread is suspended fails with the deadlock cause of [PDD018],
because nothing can resume that thread. The `run_concurrent` entry returns
pending in that state, as [PDD018] states. The seam's budget applies to nested
turns only.

## The Browser Backend Admits a Re-entrant Call

[PDD020] records a difference between the targets. A native engine enters a host
function that is already on the stack, and the browser backend refuses the
second call. This design removes the difference in the backend.

The refusal comes from a guard. The vendored `js_wasm_runtime_layer` wraps each
host function's body in a shared JavaScript closure and in a guard that detects
a second entry. The patch that introduced the guard states that the shared
closure is entered at any depth. The function the runtime layer hands the
backend is `Fn`, so the body needs no exclusive access. A host function that
calls a guest, which calls a different host function, already derives the store
from its pointer twice, on both targets. A second entry into the same function
adds no new kind of access.

The backend therefore drops the guard and calls the body directly. The browser
enters a host function at any depth, with or without a provider. Every shape
that [PDD020] lists runs the same on both targets. `SchedulerCause` loses its
re-entrant host-call variant, and the backend's refusal type goes with it. The
change breaks the public API, which the project accepts before its first major
release.

## Translation

The translator accepts `canon lift async` without `callback`, and all eight
thread built-ins, on every target. The gates that `EngineConfig` already has
decide it, as in Wasmtime: `wasm_component_model_async_stackful` for the lift,
and `wasm_component_model_threading` for the thread built-ins. Both stay off by
default. A component that uses either feature with its gate off fails with
`Error::Unsupported`, as now.

The translator never asks whether a provider exists. A component that can block
but never does runs everywhere. [PDD021] made the same choice for `task.cancel`,
for the same reason.

## Error Model Changes

- The stack-switch cause keeps its meaning: the reference permits the block and
  the target has no provider to serve it. Rule 2 of The Cause of a Failed Block
  is a new case of that meaning.
- The re-entrant host-call cause is removed.
- The thread built-ins and the stackful lift add Wasmtime's messages for their
  traps: "async-lifted export failed to produce a result", "cannot resume thread
  which is not suspended", and the two start-function messages of
  `thread.new-indirect`. Each is a structured cause of `wcmp::Error`, so the
  corpora match it by substring.

## Revisions to Earlier Designs

This design revises four statements of [PDD018]:

- The native target has a provider where the engine implements the
  stack-switching proposal.
- The provider of each target is designed here.
- Under a provider, guest code is entered from inside a trampoline only as a
  nested start, on a stack of its own above the trampoline.
- The capability is filled in every browser that ships JSPI, which is every
  current browser.

This design revises three statements of [PDD020]:

- A task that must not block reaches the provider, only to resume the threads of
  its own instance.
- A callee of an asynchronous lower starts at once from inside the trampoline,
  with or without a provider.
- The target difference of the re-entrant host call no longer exists.

[PDD003] is an inventory of upstream facts, and it states that it is the
document to revisit when an upstream source changes. Its JSPI row, its
stack-switching row, its row for the stackful lift, its row for the thread
built-ins, and its list of what it does not commit to are corrected in place.

## Target Differences

The differences of [PDD018] and [PDD020] stay, except the re-entrant host call,
which this design removes. The provider is the difference this design adds:

- Native: the stack-switching provider on an engine that implements the
  proposal. Under Wasmtime 49, that is x86_64 Linux. Nested turns elsewhere.
- Browser: the JSPI provider in every browser that ships JSPI. Nested turns in
  an older browser, such as Safari 26.

A resumption is synchronous under the stack-switching provider and runs on a
microtask under the JSPI provider. The order of the scheduler is the same under
both. A guest observes no difference, because a turn that resumes a thread runs
nothing else until the thread suspends or finishes.

## Why No Guest Rewrite

A rewrite of the guest is the one route to a stack switch that works on any
engine. Binaryen's Asyncify is the known form. Its author reports that a binary
grows "around 50% larger on average", and up to about twice its size on real
programs ([Asyncify]). A slowdown of the same order follows. A function of a
large program slowed by five times. Indirect calls push the cost toward the
worst case. Every function that can be on the stack at a suspension must be
rewritten. Here that includes the fused adapters and the core modules of every
component in a chain. The rewrite needs a port of the pass to Rust, or Binaryen
shipped to the browser. The JSPI provider serves every current browser at no
such cost. A browser without JSPI keeps nested turns.

## The Corpus

The shared expected-failure list records the best case: the failures under a
provider. The native lane runs the stack-switching provider on the x86_64 Linux
host, and the web lane runs the JSPI provider in the flake's Chromium. Each line
this design owns leaves the shared list once it passes under a provider.

A new overlay, `expected-failures.no-provider.txt`, lists the lines that fail
without a provider. Each of its lines carries the stack-switch reason. The
harness applies it when the engine's query answers that no provider exists. That
covers a lane with the provider turned off, a native build on a platform without
the proposal, and a browser without JSPI. The nested turn is one code path on
both targets, so one overlay serves both. The web overlay keeps what only the
browser does differently, and the harness applies both overlays when both hold.

The lanes with the provider off run the conformance corpus only. A repository
test that proves the fallback turns the provider off through `EngineConfig` and
runs in the ordinary lanes. `tests all` runs the corpus in all four states.
`tests regenerate` writes the shared list from the native lane with the provider
and writes the overlay from the native lane without it. This set of lanes is a
start. The owner reviews it once the time each lane takes is known.

The directives this design owns, in the Component Model corpus:

- The stackful lift: seven directives of `big-interleaving-test.wast` (912, 973,
  1295, 1508, 1623, 1640, 1651), `sync-barges-in.wast:323`, and
  `values/variants.wast:186`.
- The thread built-ins: `during-sync-call-exclusive-resume.wast`,
  `during-sync-call-may-block-if-other-ready-threads.wast`,
  `during-sync-call-no-sibling-resume.wast`,
  `during-sync-scheduling-candidates.wast`, `self-switch-traps.wast`,
  `switch-to-ready-callback.wast`, `trap-if-block-and-sync.wast`,
  `trap-if-sync-and-waitable-set.wast`, `reentrance.wast:557` and `:701`, and
  `values/post-return.wast:4`.
- A frame below: `async-calls-sync.wast:250` and `:251`,
  `cancel-and-exclusive-lock.wast:196`, and `sync-streams.wast:208`.

In the Wasmtime corpus:

- The stackful lift: `stackful.wast`, `drop-waitable-set-stackful.wast`,
  `task-deletion.wast`, and the four stackful directives of
  `task-return-traps.wast` (93, 106, 121, 138).
- The thread built-ins: the two directives of `task-return-traps.wast` that
  extract a table (21, 58), `join-during-sync-read.wast`, and
  `thread-transparency/reentrancy.wast`.
- A frame below: `reenter-during-yield.wast:81`, `stream-zero-ops.wast:201`,
  `streams-massive-send.wast:238` and `:240`, `sync-streams.wast:186`,
  `task-builtins.wast:723`, and seven directives of `trap-if-done.wast` (594,
  596, 612, 614, 623, 625, 627).

The cascade lines of each owned file go with it. A line of an owned file whose
failure has another cause keeps that cause and stays in the shared list. Those
are the two `subtask.cancel` directives of `big-interleaving-test.wast` (1664,
1675), and `reentrance.wast:548` and `:891`.

Without a provider, every directive of the frame-below group fails with the
stack-switch cause, so each is in the overlay. Every other owned directive is in
the overlay if and only if a live run without a provider fails it. A live run
sets the overlay. The design does not predict it.

## User Stories

A developer runs a C component that uses pthreads in a browser page.

> The C library starts each pthread with `thread.new-indirect` and parks it with
> `thread.suspend`. In Safari 27, the polyfill selects the JSPI provider, and
> each thread runs on a stack of its own. A thread that waits for a lock
> suspends, and the page stays responsive. In Safari 26, the same component
> loads. Its threads that never wait run to their end. The first wait that only
> a suspended thread can release fails with the stack-switch cause, and the page
> tells the person that the browser is too old.

A developer calls a synchronous export that calls an `async` host function which
fetches over the network.

> The guest lowers the host function synchronously. The shim finds the fetch
> pending and suspends the guest's thread. The driver returns pending to the
> executor, and the page or the native executor runs on. When the response
> arrives, a turn lowers it, resumes the thread, and the export returns its
> result. On a target with no provider, the same call fails with the
> stack-switch cause, because only a suspension can wait for the network.

A developer on an ARM laptop runs the conformance suite natively.

> The engine's query answers that no provider exists, because Wasmtime
> implements the proposal on x86_64 Linux only. The harness applies the
> no-provider overlay, and the run is green with the lines it lists.

A contributor adds a provider for a new engine.

> The contributor finds one contract with six duties, one switch module, and one
> probe. If the engine implements the stack-switching proposal, the contributor
> has nothing to add. If it has another way to switch stacks that meets the
> contract, the contributor adds a provider beside the two here.

## Test Cases

The switch probe selects the provider. On the x86_64 Linux host, the native
engine's query answers the stack-switching provider. In the flake's Chromium,
the query answers the JSPI provider. With the provider turned off through
`EngineConfig`, the query answers none on both targets. A repository test proves
each answer.

The stack-switching provider meets the contract next to the fused adapters. A
repository test starts a stackful export of one component that calls a second
component through a fused adapter, suspends in the second, resumes, and returns.
A second test starts a thread by nested start from inside a host trampoline,
suspends it, lets the trampoline return, and resumes the thread after its
starter. Both pass natively on the x86_64 Linux host.

The JSPI provider meets the contract in the browser. The same two repository
tests pass in the web lane. A third test proves that a shim whose built-in is
ready returns without a suspension.

A synchronous export calls an `async` host function. A repository test lowers a
host function whose future is pending for two polls. With a provider, the export
returns the host function's result on both targets. With the provider off, the
call fails with the stack-switch cause.

A readiness condition runs no guest code. A repository test blocks a thread on a
host task whose body runs guest work through its accessor. With a provider, the
thread resumes after a turn polls the body. No nested turn runs, and the
provider stays in the store for the whole suspension.

A task that must not block switches only within its instance. With a provider,
`during-sync-call-may-block-if-other-ready-threads.wast`,
`during-sync-call-no-sibling-resume.wast`, and
`during-sync-scheduling-candidates.wast` pass whole on both targets.
`trap-if-block-and-sync.wast` passes whole with the cannot-block message.

The stackful lift runs. With a provider, `stackful.wast`,
`drop-waitable-set-stackful.wast`, `task-deletion.wast`, `sync-barges-in.wast`,
`values/variants.wast`, the seven stackful directives of
`big-interleaving-test.wast`, and the four stackful directives of
`task-return-traps.wast` pass on both targets, with the message "async-lifted
export failed to produce a result" where the file expects it.

The thread built-ins run. With a provider, `self-switch-traps.wast`,
`switch-to-ready-callback.wast`, `trap-if-sync-and-waitable-set.wast`,
`during-sync-call-exclusive-resume.wast`, `join-during-sync-read.wast`,
`thread-transparency/reentrancy.wast`, `reentrance.wast:557` and `:701`,
`values/post-return.wast`, and the two table directives of
`task-return-traps.wast` pass on both targets. A repository test proves the
`i64` start function of `thread.new-indirect` in a 64-bit memory.

A frame below releases a block. With a provider, the frame-below directives of
both corpora pass on both targets: `async-calls-sync.wast:250` and `:251`,
`cancel-and-exclusive-lock.wast:196`, both `sync-streams.wast` files,
`reenter-during-yield.wast:81`, `stream-zero-ops.wast:201`,
`streams-massive-send.wast:238` and `:240`, `task-builtins.wast:723`, and the
seven directives of `trap-if-done.wast`.

The cause of a failed block is true. With the provider off, every frame-below
directive fails with the stack-switch message, and none with the deadlock
message. `deadlock.wast`, `wait-forever.wast`, and `wait-forever2.wast` still
fail with the deadlock message on both targets, with and without a provider. A
repository test proves rule 2 of The Cause of a Failed Block, and the existing
repository tests still prove rules 1, 3, and 4.

The browser enters a host function at any depth. A repository test calls the
same host import from inside that import's own call, in the web lane, and the
call succeeds. A second test drops a second handle of a resource type from
inside that type's destructor in the browser, and the drop succeeds. The
re-entrant host-call cause no longer exists in the public API.

A store drops its suspended threads. A repository test drops a store while a
thread of a stackful export is suspended, under each provider. The drop
completes, no destructor runs, and a later store on the same engine runs the
same export to its end.

The lists measure both states. The shared list holds no line this design owns,
except the lines that fail for another cause. Every line of
`expected-failures.no-provider.txt` carries the stack-switch reason. `tests all`
runs the corpus in all four states and each state is green against its lists.

## References

- [PDD003], the compatibility outlook, corrected in place by this design.
- [PDD016], the conformance suite.
- [PDD018], the concurrency runtime model, whose suspend capability this design
  fills.
- [PDD019], tasks and the callback export.
- [PDD020], subtasks and the async import, whose nested turn stays the fallback.
- [PDD021], streams and futures, whose synchronous copies block under a
  provider.
- [Concurrency], the Concurrency explainer, and its sections on [threads and
  tasks][Concurrency – threads], [thread
  built-ins][Concurrency – thread built-ins],
  [blocking][Concurrency – blocking], and [stackful
  exports][Concurrency – stackful].
- [CanonicalABI – thread built-ins], the eight built-ins as the Canonical ABI
  explainer states them.
- [`definitions.py`], the executable reference for `Thread`, `Task`,
  `canon_lift`, and the thread built-ins.
- [Wasmtime], the reference implementation at `v49.0.0-rc.1`: its [trap
  messages][Wasmtime traps], its [may-not-suspend
  rule][Wasmtime may-not-suspend], its [activation state][Wasmtime activations],
  and its [stack-switching support][Wasmtime stack switching].
- [JSPI], the JavaScript Promise Integration proposal.
- [caniuse], the browser support table for JSPI, and [Safari 27.0], the release
  that added it to Safari.
- The [stack-switching proposal].
- wasm3's [stack-switching support][wasm3 stack switching] and its [native stack
  limit][wasm3 stack limit].
- wit-bindgen's [core ABI][wit-bindgen ABI] and its [C
  generator][wit-bindgen C].
- [Asyncify], the report of Binaryen's author on the cost of a guest rewrite.
- The [Component Model test corpus] and the [Wasmtime component tests].

[PDD003]: ./PDD003%20Compatibility%20Outlook.md
[PDD016]: ./PDD016%20Conformance%20Suite.md
[PDD018]: ./PDD018%20Concurrency%20Runtime%20Model.md
[PDD019]: ./PDD019%20Tasks%20and%20the%20Callback%20Async%20Export.md
[PDD020]: ./PDD020%20Subtasks%20and%20the%20Async%20Import.md
[PDD021]: ./PDD021%20Streams%20and%20Futures.md
[Concurrency]:
  https://github.com/WebAssembly/component-model/blob/main/design/mvp/Concurrency.md
[Concurrency – threads]:
  https://github.com/WebAssembly/component-model/blob/main/design/mvp/Concurrency.md#threads-and-tasks
[Concurrency – thread built-ins]:
  https://github.com/WebAssembly/component-model/blob/main/design/mvp/Concurrency.md#thread-built-ins
[Concurrency – blocking]:
  https://github.com/WebAssembly/component-model/blob/main/design/mvp/Concurrency.md#blocking
[Concurrency – stackful]:
  https://github.com/WebAssembly/component-model/blob/main/design/mvp/Concurrency.md#stackful-async-exports
[CanonicalABI – thread built-ins]:
  https://github.com/WebAssembly/component-model/blob/main/design/mvp/CanonicalABI.md#-canon-threadindex
[`definitions.py`]:
  https://github.com/WebAssembly/component-model/blob/main/design/mvp/canonical-abi/definitions.py
[Wasmtime]: https://github.com/bytecodealliance/wasmtime
[Wasmtime traps]:
  https://github.com/bytecodealliance/wasmtime/blob/v49.0.0-rc.1/crates/environ/src/trap_encoding.rs
[Wasmtime may-not-suspend]:
  https://github.com/bytecodealliance/wasmtime/blob/v49.0.0-rc.1/crates/wasmtime/src/runtime/component/concurrent.rs#L2047-L2073
[Wasmtime activations]:
  https://github.com/bytecodealliance/wasmtime/blob/v49.0.0-rc.1/crates/wasmtime/src/runtime/vm/traphandlers.rs#L1206-L1215
[Wasmtime stack switching]:
  https://github.com/bytecodealliance/wasmtime/blob/v49.0.0-rc.1/crates/cranelift/src/func_environ/stack_switching/instructions.rs#L13-L20
[JSPI]:
  https://github.com/WebAssembly/js-promise-integration/blob/main/proposals/js-promise-integration/Overview.md
[caniuse]: https://caniuse.com/wf-wasm-jspi
[Safari 27.0]: https://webkit.org/blog/18325/webkit-features-for-safari-27-0/
[stack-switching proposal]: https://github.com/WebAssembly/stack-switching
[wasm3 stack switching]:
  https://github.com/wasm3/wasm3/blob/6b39554/source/m3_config.h#L307-L324
[wasm3 stack limit]:
  https://github.com/wasm3/wasm3/blob/6b39554/source/m3_env.h#L670-L684
[wit-bindgen ABI]:
  https://github.com/bytecodealliance/wit-bindgen/blob/2f795ab/crates/core/src/abi.rs#L1350-L1351
[wit-bindgen C]:
  https://github.com/bytecodealliance/wit-bindgen/blob/2f795ab/crates/c/src/lib.rs#L738-L761
[Asyncify]: https://kripken.github.io/blog/wasm/2019/07/16/asyncify.html
[Component Model test corpus]:
  https://github.com/WebAssembly/component-model/tree/main/test/async
[Wasmtime component tests]:
  https://github.com/bytecodealliance/wasmtime/tree/main/tests/misc_testsuite/component-model/async
