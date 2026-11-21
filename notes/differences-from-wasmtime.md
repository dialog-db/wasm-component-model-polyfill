# Differences from Wasmtime

The polyfill mirrors Wasmtime's component API and runs Wasmtime's own component
tests. A host can notice the differences below.

## Blocking needs a suspend provider

Wasmtime runs each guest on a fiber, so a guest can always block in the middle
of a call. The polyfill needs a [suspend provider](suspend-providers.md) for
that. Without one, a block that only a stack switch can serve fails with
`SchedulerCause::StackSwitchNeeded`.

## A trap poisons the store

As in Wasmtime, one trap poisons the whole store. Every later call into guest
code fails with "cannot enter component instance", and a host recovers by
building a new store. Work that runs no guest code, such as reading host data or
dropping the store, still works. The polyfill is stricter than Wasmtime in two
ways:

- It refuses to instantiate into a poisoned store, because instantiation runs
  guest start functions.
- It drops all queued guest work and pending host futures at the moment of the
  trap. Wasmtime keeps them for a later `run_concurrent`.

## A dropped queued call still runs

A call into a component can wait in a queue before it starts, for example while
the callee instance has backpressure set. If the host drops the future of such a
call, Wasmtime 50 makes the call trap when it starts, and the trap poisons the
store. A `TODO` in Wasmtime's source asks to skip such a call instead
(`host_future_dropped` in `concurrent.rs` at `v50.0.0`). The polyfill starts the
call when its turn comes, because the call owns its arguments and needs nothing
from the dropped future. A trap in that call still poisons the store, and the
next driver of the store reports it.

## A lifted NaN is canonical

The Component Model makes every NaN that a lift produces the one canonical NaN
(`0x7fc00000` as an `f32`, `0x7ff8000000000000` as an `f64`). The polyfill
follows it, so a host reads those bits whatever NaN a guest gave it. Wasmtime
keeps the guest's payload. A guest sees no difference, since a lower may hand it
any NaN.

## The first thread of a call is never refused

A store caps the handles it holds at once, as in Wasmtime 50, and each guest
thread counts as one handle. When the store is at its cap, Wasmtime refuses the
thread that a new call starts on, so the call traps. The polyfill counts that
thread but does not refuse it, so the call starts. Every other new handle is
refused at the cap, as in Wasmtime, and that includes a thread that
`thread.new-indirect` makes.

## The memories of an instance

Wasmtime gives a host no read of the core memories inside a component instance.
Its nearest API is for debuggers: `Store::debug_all_instances` and
`Instance::debug_memory`, which need `Config::guest_debug`. The polyfill lists
the memories with `Instance::core_memories`, and needs no setting of the engine.
A host reads the size of each one as it reads a `wasmtime::Memory`: in pages
with `Memory::size`, and in bytes with `Memory::data_size`. A host can notice
three differences:

- The list holds only the memories that a core module defines and exports. The
  browser's engine reaches only the exports of an instance. A memory that the
  Canonical ABI reads and writes is always exported, because the component names
  it by an export.
- `Memory` reads the size and nothing else. It has no `data`, `read`, `write`,
  or `grow`, because the component model moves values across the boundary
  through lifted functions.
- `Memory::size` and `Memory::data_size` take the `Store` and return a `Result`.
  A read through another store fails with `InstantiationError::WrongStore`.
  Wasmtime's methods take any context of the store and panic on another store.
