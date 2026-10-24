# Context Slots in Globals

A thread of a component has two context slots, which `context.get` and
`context.set` read and write. [PDD019] keeps them on the thread record. This
document designs how a guest reaches them with no call of the host.

A guest that Rust builds for `wasm32-wasip3` keeps its shadow stack pointer in
context slot 0. Most of its functions read the slot on entry and write it on
entry and on exit. When each built-in is a host function, a guest pays two or
three host calls for each ordinary function call. On the web backend each host
call also crosses into JavaScript.

Four terms recur:

- The context module is a core module that the polyfill generates. It holds the
  slots of one thread in WebAssembly globals.
- The loaded flag is a global of the context module. It says whether the slot
  globals hold the slots of a thread.
- The owner is the thread whose slots the globals hold.
- To write back is to copy the slot globals into the owner's record and to clear
  the loaded flag.

## Goals

- A read or a write of a loaded slot runs in WebAssembly alone.
- Each thread keeps its own slots, as [PDD019] states.
- Host code that reads a thread's record sees what the thread last wrote.
- A host call that returns to the same thread does not unload the slots.
- Every backend runs the same design.

## Non-goals

- This design changes no rule of [PDD019]. The slots keep their lifecycle.
- This design does not move the slots off the thread record.

## The Context Module

Each store has one context module. The first component of the store that imports
a context built-in instantiates it. Every later component reuses it. The engine
compiles the module once, and each store of the engine instantiates that
compiled module.

The module holds three mutable `i32` globals: slot 0, slot 1, and the loaded
flag. It exports `get0`, `set0`, `get1`, and `set1`, which are the built-ins of
every component in the store. It imports one host function, `load`.

```text
getN:
    if loaded == 0: call load
    return slot N

setN(value):
    if loaded == 0: call load
    slot N = value
```

`load` copies the slots of the current thread into the globals. It sets the
loaded flag and records that thread as the owner. With no task on the stack,
`load` fails, as the built-in of [PDD019] fails there.

## Writing Back

The current thread changes only while the host runs. The store's call hook keeps
the slots right at each crossing between the host and guest code. The engine
calls the hook at each crossing and tells it which crossing it is, as [PDD025]
states. The hook does one of three things:

- When the guest calls a host function, the hook copies the slot globals into
  the owner's record. The slots stay loaded. So host code that reads the record
  sees what the thread last wrote.
- When a host function returns to the guest, the hook reads the current thread.
  If the owner is still the current thread, the guest that called the function
  goes on, and the slots stay loaded. If not, the hook writes back.
- When the host calls guest code, and when guest code returns to the host, the
  hook writes back.

`load` is a host function too, so the hook runs around it. When the guest calls
`load`, nothing is loaded. When `load` returns, its owner is the current thread,
so the slots stay loaded.

A host function can switch threads. Then the owner is not the current thread
when the function returns, and the hook writes back. A host function can also
run guest code of another thread. The call of that guest code writes back first,
so the other thread loads its own slots.

The design depends on one rule. Guest code reads or writes the globals without
`load` only while the slots are loaded and their owner is the thread that runs.
Each crossing to guest code of another thread writes back first. So no thread
sees the slots of another thread.

Two rules of the rest of the polyfill hold this up:

- The current thread names the thread that a host function returns into. `load`
  needs this rule too.
- No host code writes the slots of a thread whose slots are loaded. The globals
  would overwrite the write at the next write back.

In the browser, a guest that suspends in a suspending host function keeps its
slots loaded until the host sees the stop, a microtask later. No other guest
code of the store runs in that time, because the store lets one call run at a
time.

A host function can run guest code whose wait the host drops. That guest code
does not return to the host. If its thread loaded slots, the owner is not the
current thread when the host function returns, so the hook writes back then.

A write back clears the loaded flag before it forgets the owner. If the
substrate fails during a write back, the slots stay loaded for their owner.

When the host drops the wait of a call, the record of the call's thread is stale
until the next crossing writes back. Only `load` reads the slots of a record,
and it runs after a crossing, so no host code sees the stale record.

After a write back, the next thread that reads or writes a slot loads its own. A
thread that ended before a write back has no record, and its slots end with it.

## Revisions to Earlier Designs

[PDD025] is revised in place. A store holds a call hook.

## User Stories

As a host author, I run a component built for `wasm32-wasip3`, and its ordinary
function calls make no host calls for the shadow stack pointer.

As a host author, I instantiate several components in one store, and each thread
of each component sees only its own slots.

## Test Cases

A thread of each of two components in one store starts with zero slots. A test
instantiates one component twice in a store. Its export answers what slot 0 held
and sets it. Each call answers zero, whichever component ran before.

Each thread of one component keeps its own slots. A test starts a thread from an
export, and the export yields to it. The thread finds zero slots, writes its own
value, and calls the host. After the call, it finds its value. When the thread
ends, the export finds the value that it wrote before the switch.

A host function reads what a guest wrote. A guest writes slot 0 and calls the
host. The host reads the value from the record of the current thread. After the
call returns, the guest finds its value in slot 0.

The polyfill loads and writes back the slots of the current thread. A test
pushes two tasks. Each task finds zero slots and writes its own. After the inner
task ends, the outer task finds what it wrote.

The rules of [PDD019] hold. The repository tests of context slots across
reallocs, destructors, composed calls, and callback tasks pass unchanged.

## References

[PDD019]: ./PDD019%20Tasks%20and%20the%20Callback%20Async%20Export.md
[PDD025]: ./PDD025%20Runtime%20Layer.md
