# Suspend providers

Wasmtime runs each guest on a fiber, so a guest can always block in the middle
of a call. The polyfill runs on the one stack that its target gives it, so it
needs a suspend provider to pause a guest and resume it later. The engine picks
a provider when it is built, from the features that its backend declares.
`Engine::suspend_provider()` returns the choice:

| Backend                                                                                                                 | Provider                           |
| ----------------------------------------------------------------------------------------------------------------------- | ---------------------------------- |
| Wasmtime where it serves stack switching and the switch probe passes: x86_64 Linux (tested) and x86_64 macOS (untested) | Stack switching (`StackSwitching`) |
| The browser backend, in a browser with JSPI (every current browser)                                                     | Host suspension (`HostSuspension`) |
| Wasmi, on any platform                                                                                                  | Host suspension (`HostSuspension`) |
| Wasmtime on other platforms, the browser backend without JSPI (such as Safari 26), or `suspend_provider(false)`         | None (`None`)                      |

Sources: the crate docs of each backend, and
`rust/wcmp/src/suspend_provider_kind.rs` for the provider. The Wasmtime crate
docs name x86-64 Linux and macOS as the platforms where it declares
`stack_switching`. The polyfill's switch probe passes on x86_64 Linux.

## Without a provider

A blocked guest runs the store's other work in nested turns until it can go on,
and most blocks need nothing more. A block that only a stack switch can serve,
such as a synchronous call to a host `async` function whose future is still
pending, fails with `SchedulerCause::StackSwitchNeeded` instead of hanging.
[Conformance](conformance.md) shows how many directives that costs.

## Host suspension

Host suspension lets a host function answer "not yet". The engine sets the guest
call aside, and the host resumes it later with the result. The polyfill uses it
to pause a guest thread.

|                              | Browser                                                    | Wasmi                                           | Wasmtime                                                                                |
| ---------------------------- | ---------------------------------------------------------- | ----------------------------------------------- | --------------------------------------------------------------------------------------- |
| Mechanism                    | JSPI: `WebAssembly.promising` and `WebAssembly.Suspending` | Wasmi's resumable calls (`call_resumable`)      | None. The polyfill uses stack switching instead, where the backend declares it          |
| A resumption                 | Runs later, on a microtask                                 | Runs at once, to the next suspension or the end | Not applicable                                                                          |
| Suspended calls in one store | Any number, resumed in any order                           | Any number, resumed in any order                | Not applicable. `Func::call_async` holds the store, so one store has one suspended call |

## Limits of host suspension

- In the browser and on Wasmi, a call can suspend only where WebAssembly frames
  alone lie between the start of the resumable call and the host function. Where
  a host frame lies between them, "not yet" traps the guest with
  `TrapKind::Host` (each crate's docs).
- In the browser, a host function cannot resume a call, because the browser runs
  the resumed stack only after the host function returns. The attempt fails with
  `Error::Backend` (crate docs).
- In the browser, a guest thread can start in place inside a host function,
  during a guest call that cannot suspend. If the thread traps before it first
  pauses, the trap has no reason from the engine. The browser reports the reason
  only to a caller that awaits the call, and a host function cannot await. This
  costs one conformance directive,
  `wasmtime/thread-transparency/reentrancy.wast:114`
  (`rust/wcmp/tests/corpus/expected-failures.web.txt`, card `656c71`).
- In the browser, a host function used as a guest thread's start function fails,
  and so does a guest that blocks inside a destructor, a `post-return` function,
  or a core start function. No conformance test reaches these cases.
- On Wasmi, a start and a resumption both finish inside the poll that begins
  them, since each future is ready on its first poll (crate docs). A long guest
  computation holds the thread that polls until it suspends or ends.
