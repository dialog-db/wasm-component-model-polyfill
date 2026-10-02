# Zena TodoMVC Demo

A TodoMVC application whose elements and routes are Zena programs. The page and
the service worker each run Zena's compiler as a component on the polyfill, and
compile the Zena sources when the demo starts. A person can edit any source in
the drawer and see the change at once.

- `src/` is the host framework: the compiler per context, the custom elements,
  the routes, `wasi:http`, the todo model, and the drawer. `src/bin/` holds the
  two Trunk binaries, one for the page and one for the service worker.
- `zena/authoring/` is the authoring library, the package `authoring` of the
  source bundle. Authors import `authoring:element` and `authoring:route`.
- `zena/app/` holds the demo's elements and routes.
- `zena/wit/` holds the element world and the route world. Authors never write
  WIT.
- `web/` holds the page, the service worker's JavaScript entry, the element
  shim, and the styles.

`demo serve` builds the demo and serves it on a loopback port. `tests demo` runs
the browser test lane in `rust/wcmp-demo-lane`.

## Tracing

The page and the service worker each record the spans of the polyfill, the
runtime layer, and the demo with `tracing-web`. Each span becomes user timing
marks and a measure, named after the span. Record a profile in the browser's
performance panel and open the timings track to see them: the Zena compiles,
the element renders and their diffs, the routes, the todo model, IndexedDB, and
under them the polyfill's compiles, instantiations, and calls. Events at `INFO`
and above go to the console.

The spans are at `DEBUG` and above by default. The `trace` parameter of the
page's URL names another level: `?trace=trace` adds the polyfill's turns, its
canonical ABI, and its host calls, which are many, and `?trace=off` records
none. The page hands the parameter to the service worker. A service worker
keeps its own timeline: to see its spans, profile the worker itself, in the
DevTools that `chrome://inspect/#service-workers` opens for it. Each context
clears its
timeline every five seconds, so that the marks do not pile up while nothing
records them.

## Zena Defects the Demo Works Around

The demo works around these Zena defects. Each one is a candidate for a report
to Zena.

1. A WIT variant named `error` clashes with Zena's built-in `Error`. The todo
   model names its variant `model-error`.
2. A class field typed `Array<T>` with a `GrowableArray` initializer fails in
   the code generator with "constructor field type". The fields use `var` with
   no declared type.
3. A call to `h('slot')` that relies on default arguments, inside a method of a
   base class, fails in the code generator with "call argument type". The base
   class builds its `View` with every argument.
4. An imported function cannot be used as a value ("non-local identifier"). The
   glue wraps each import in a closure.
5. The code generator does not support the `unreachable()` intrinsic. The
   authoring library traps by reading through a null reference.
6. An exception that escapes an async export with no result returns normally,
   with no trap. The authoring library catches the exception and traps itself.
7. Code after a `throw` in the same block fails with "Unreachable code
   detected", so a test fixture that throws needs a condition around the
   `throw`.
