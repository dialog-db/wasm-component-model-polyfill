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
- Every backend runs the same design.

## Non-goals

- This design changes no rule of [PDD019]. The slots keep their lifecycle.
- This design does not move the slots off the thread record.

## The Context Module

Each store has one context module. The first component of the store that imports
a context built-in instantiates it. Every later component reuses it.

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

The current thread changes only while the host runs. So the polyfill writes back
each time control can pass to the host. The store's call hook does this. The
engine calls the hook at each crossing between the host and guest code, and
tells the hook which crossing it is, as [PDD025] states.

The hook writes back at each crossing but one: the return of a host function to
the guest that called it. The call of that function wrote back already. So when
the function returns, only the slots that `load` loaded for the guest are in the
globals, and they stay there.

`load` is a host function too, so the hook runs around it. When the guest calls
`load`, the hook writes back, and nothing is loaded then. When `load` returns,
the hook keeps the slots that `load` loaded.

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
