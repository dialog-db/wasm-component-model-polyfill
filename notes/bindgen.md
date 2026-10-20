# Host bindings with `bindgen!`

`wcmp::bindgen!` reads WIT and writes the Rust code that connects a host to a
component. It uses the names and the workflow of Wasmtime's
`wasmtime::component::bindgen!`, and the code it writes calls the polyfill's
API. The macro runs at compile time on the build machine, so no browser binary
contains its WIT parser. It writes the same code for every target.

This note shows how to use the macro. The last section lists each difference
from Wasmtime's macro, with its reason. The crate docs of `wcmp::bindgen!` are
the full reference.

## Invoke the macro

The macro takes Wasmtime's five forms. A path is relative to the `Cargo.toml` of
the crate that invokes the macro.

| Form                          | What it reads                                   |
| ----------------------------- | ----------------------------------------------- |
| `bindgen!()`                  | `wit/`, which holds one world                   |
| `bindgen!("world")`           | `wit/`, and the world that the string names     |
| `bindgen!(in "path")`         | the directory or file at `path`, with one world |
| `bindgen!("world" in "path")` | the path, and the world that the string names   |
| `bindgen!({ … })`             | the options in the braces                       |

Put the invocation in a module of its own, so that the generated items stay
apart from your code:

```rust
mod bindings {
    wcmp::bindgen!("app");
}
```

## The code the macro writes

Each WIT interface becomes a module at Wasmtime's path. The interface
`example:app/log` becomes `bindings::example::app::log`, and an exported
interface sits under `bindings::exports`. Each WIT type becomes a Rust type with
the derives `ComponentType`, `Lift`, and `Lower`, and a WIT `flags` type becomes
a `flags!` type. An imported resource becomes an empty enum, and its handle is
`Resource<Name>`.

Each imported interface has these items:

- The trait `Host`, with one method for each function. You implement it on your
  host data.
- The trait `HostWithStore<T>`, with one method for each function that needs the
  store. These are the functions with the `store` flag and every WIT
  `async func`.
- The functions `add_to_linker_instance` and `add_to_linker`, which register
  each function in a `Linker`.

The world becomes a struct with the world's name. The struct has
`add_to_linker`, `instantiate_async`, `new`, and one getter for each exported
interface. Each getter gives a struct `Guest` with a `call_*` method for each
function of that interface.

## Serve an import and call an export

This WIT imports a log and exports a greeting:

```wit
package example:app;

interface log {
  write: func(message: string);
}

interface greet {
  hello: func(name: string) -> string;
}

world app {
  import log;
  export greet;
}
```

The host implements `Host` on its own data, registers the world, and calls the
export:

```rust
use bindings::App;
use bindings::example::app::log;
use wcmp::{Component, Engine, HasSelf, Linker, Store};

struct HostData {
    lines: Vec<String>,
}

impl log::Host for HostData {
    fn write(&mut self, message: String) {
        self.lines.push(message);
    }
}

async fn greet(engine: &Engine, component: &Component) -> wcmp::Result<String> {
    let mut linker = Linker::new(engine);
    App::add_to_linker::<HostData, HasSelf<HostData>>(&mut linker, |data| data)?;
    let mut store = Store::new(engine, HostData { lines: Vec::new() })?;
    let app = App::instantiate_async(&mut store, component, &linker).await?;
    app.example_app_greet().call_hello(&mut store, "world").await
}
```

`HasSelf<HostData>` and the closure `|data| data` are the projection. A
projection names the part of the host data that serves an interface, and
`HasSelf` names the whole host data. A crate that serves an interface for other
crates names a view type of its own instead, as Wasmtime's WASI crates do.

## Async functions

An imported WIT `async func` is a method of `HostWithStore<T>`. The method takes
an `&Accessor<T, Self>` and returns a future. An accessor is a token that
reaches the store only inside a call to `with`, so the future can keep it across
an `.await`. In this example, `sleep` stands for any timer future:

```rust
impl clock::HostWithStore<HostData> for HasSelf<HostData> {
    async fn wait(accessor: &Accessor<HostData, Self>, ms: u32) {
        sleep(ms).await;
        accessor.with(|mut access| access.get().lines.push(format!("waited {ms} ms")));
    }
}
```

An exported WIT `async func` has a `call_*` method that takes an
`impl AsAccessor`. Call it inside `Store::run_concurrent`, which gives the
accessor. For an exported function `fetch`:

```rust
let body = store
    .run_concurrent(async |accessor| calls.call_fetch(accessor, url).await)
    .await?;
```

## Resources

An imported resource `counter` has the trait `HostCounter`, with its
constructor, its methods, and `drop`. The host keeps each value in a
`ResourceTable` and gives the guest a `Resource<Counter>`. An exported resource
has a getter on the interface's `Guest`, such as `counter()`. The calls of that
getter take the `ResourceAny` that the guest gave the host.

## Errors

A function with the `trappable` flag returns `wcmp::Result<T>`, and an `Err`
traps the guest. The option `trappable_error_type` maps a WIT error type to a
Rust error of your own. The method then returns `Result<T, YourError>`, and a
method `convert_*` turns your error into the WIT error. With `anyhow: true`, a
trappable method returns `anyhow::Result<T>`.

An error from a host function reaches the caller as `Error::Host`, which keeps
your error as its source. To find your error, call `std::error::Error::source`
on the error that the call returned, and then `downcast_ref`.

## One host on both runtimes

A host that runs on Wasmtime natively and on the polyfill in the browser keeps
its logic in a core with no runtime types. It writes one glue module for each
macro, and each glue module implements the generated traits by calling the core.
A condition on the target selects the glue module. The repository test
`rust/wcmp-bindgen-tests/tests/dual.rs` runs one guest under both glue modules,
and both record the same values.

## Differences from Wasmtime's `bindgen!`

The two macros read the same WIT and take the same options, except the options
in the second table. The first table lists the differences in the code that the
macros write.

| Difference                                                                                                       | Reason                                                                                                                                       |
| ---------------------------------------------------------------------------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------- |
| The generated code names `wcmp::…`, not `wasmtime::component::…`.                                                | The bindings call the polyfill's API. Set `wcmp_crate: path` when another crate re-exports the polyfill.                                     |
| Every `call_*` method is an `async fn`.                                                                          | A call can suspend in the browser, so every call into a component is async. On Wasmtime, `exports: { default: async }` gives the same shape. |
| A `call_*` method takes `&mut Store<T>`. Wasmtime's takes any `AsContextMut`.                                    | The polyfill's calls take the store itself. Code that passes `&mut store` is the same on both runtimes.                                      |
| `Access::store()` gives the store context. Wasmtime's `Access` is an `AsContextMut` itself.                      | The polyfill has no `AsContextMut` trait. Its store context is the type `StoreContext`.                                                      |
| The world has no `instantiate`, `{World}Pre`, or `{World}Indices`.                                               | Instantiation is async in the polyfill, and the polyfill has no pre-instantiated component.                                                  |
| There are no `LinkOptions`. Every item behind a WIT feature gate is generated as though the feature is on.       | The polyfill's linker has no switch for `@unstable` items.                                                                                   |
| `Accessor` has no `spawn`.                                                                                       | The polyfill does not run background tasks that a host function starts.                                                                      |
| `Accessor` is `Clone`, and it has `try_with` beside `with`.                                                      | The polyfill's host futures are `'static`, so each future owns a copy of the token. `try_with` returns an error where `with` panics.         |
| `require_store_data_send` asks for `T: Send` natively only.                                                      | In the browser, the generated futures and traits do not need `Send`.                                                                         |
| You find a host's own error through `source()`. Wasmtime's error has `downcast_ref` itself.                      | `wcmp::Error` keeps a host's error as the source of `Error::Host`, and has no `downcast_ref`.                                                |
| Outside the macro, `Store::new` returns a `Result`, `Component::new` is awaited, and the engine takes a backend. | The host chooses the backend that runs core WebAssembly, and the browser compiles a module asynchronously.                                   |

The macro refuses these options and flags. The compile error names the option
and the reason.

| Option or flag               | Reason                                                                                                                                                                        |
| ---------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `async` on a plain import    | The polyfill has no host function that holds the store across an await, as Wasmtime's `func_wrap_async` does. Declare the function `async func` in the WIT, or drop the flag. |
| `async` on a destructor      | A resource's destructor is synchronous in the polyfill. Use `store` to give `drop` an `Access`, or drop the flag.                                                             |
| `ownership: Borrowing { … }` | The polyfill's derived types have no lifetime parameters, so a generated type cannot borrow its fields. Use `ownership: Owning`, the default.                                 |
| `named_imports`              | It needs a linker that reads the `(implements …)` annotations of a component, and the polyfill's linker does not read them.                                                   |
| `include_component_type`     | The polyfill writes no encoded component type for its bindings.                                                                                                               |
| `wasmtime_crate`             | The generated code names `wcmp`. Use `wcmp_crate: path`.                                                                                                                      |

Both macros refuse a WIT getter or setter, a function with a `[get]` or `[set]`
name. Wasmtime 50 refuses it with "unimplemented getter/setter", and the
polyfill's compile error quotes that message.
