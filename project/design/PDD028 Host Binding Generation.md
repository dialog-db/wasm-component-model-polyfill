# Host Binding Generation

A host author who uses Wasmtime keeps WIT files in `wit/` and calls
`wasmtime::component::bindgen!`. The macro writes the Rust code that connects
the host to a component. It writes a trait for each imported interface, an
`add_to_linker` function, a struct for the world, and one Rust type for each WIT
type. The author implements the traits and calls the struct.

This design gives the polyfill a macro with the same name and the same workflow:
`wcmp::bindgen!`. The macro reads the same WIT with the same invocation. It
emits code against the polyfill's own API on every target, natively and in the
browser.

The design aims for a host that runs on both runtimes. Such a host keeps its
logic in one module that depends on no runtime. It adds one small module for
each runtime that implements the generated traits. A compile-time condition
selects the module. The design copies Wasmtime's names and shapes where the copy
is cheap. Where a copy makes the polyfill more complex, the polyfill keeps its
own shape, and this document lists the difference.

Eighteen terms recur:

- A runtime is Wasmtime or the polyfill.
- The macro is `wcmp::bindgen!`, the binding generator of the polyfill.
- Wasmtime's macro is `wasmtime::component::bindgen!`.
- Generated code is the Rust code that a binding generator emits from WIT.
  [PDD027] defines it.
- An invocation is one use of the macro, with its options.
- An option is one key of an invocation, such as `world` or `with`.
- A flag is one value that the `imports` or `exports` option gives to a
  function, such as `trappable` or `store`.
- A plain function is a WIT function declared with `func`.
- An async function is a WIT function declared with `async func`.
- The host core is the part of a host that depends on no runtime.
- A glue module is the part of a host that implements the generated traits for
  one runtime and calls the host core.
- To refuse is to fail the compile with an error that names the option and the
  reason.
- The option ledger is the list in this document that classifies each option and
  each flag of Wasmtime's macro.
- The codegen corpus is the set of WIT files that Wasmtime's macro tests use.
- A projection is a function pointer that reaches one part of the host data from
  the whole host data.
- An interface getter is a method of the world struct that gives the calls of
  one exported interface.
- A rep is the `u32` that represents one resource to its definer. [PDD009]
  defines it.
- An `eq` resource is a resource that one interface uses from another interface
  with `use`.

## Goals

- A host author generates bindings with `wcmp::bindgen!` and the invocation
  forms of Wasmtime's macro.
- The generated code has Wasmtime's names: the modules, the `Host` traits,
  `add_to_linker`, the world struct, and the interface getters.
- The macro emits the same code natively and on `wasm32-unknown-unknown`.
- A host core serves a component through a Wasmtime glue module natively and
  through a polyfill glue module on both targets.
- Each option and each flag of Wasmtime's macro has one entry in the option
  ledger.
- Every refusal is a compile error that names the option and the reason.
- The generated code uses the typed value contract of [PDD027] and does not
  change it.
- A host error from a generated method traps the guest and reaches the host's
  call with its source chain.
- The test suite runs the macro over the codegen corpus at the pinned version of
  Wasmtime.

## Non-goals

- A re-export of Wasmtime's macro. Its output names Wasmtime's `Linker`,
  `Store`, and traits, so a host that used it natively bypasses the polyfill.
- Token-identical output. Two glue modules are the expected cost of a host that
  runs on both runtimes.
- A host function that holds the store across an await, such as Wasmtime's
  `func_wrap_async`. It serves the `async` flag on a plain function. Wasmtime's
  own bindings for WASI 0.3 use that flag on socket functions.
- A deep borrow of parameters, such as Wasmtime's `ownership: Borrowing { … }`.
  It needs derived types with lifetime parameters.
- Named imports for `(implements …)` annotations, such as Wasmtime's
  `named_imports`. They need linking that reads the component's annotations.
- An encoded component type in the generated code, such as Wasmtime's
  `include_component_type`.
- Link options that turn `@unstable` features on and off, such as Wasmtime's
  `LinkOptions`.
- A pre-instantiated world, such as Wasmtime's `{World}Pre` and
  `{World}Indices`. [PDD027] excludes the pre-instantiated component that they
  rest on.
- Background tasks that a host function starts, such as Wasmtime's
  `Accessor::spawn`.
- Asynchronous resource destructors. [PDD027] excludes them.
- A redesign of the typed values, the resource handles, or `ResourceTable`.
  [PDD027] designs them.

## Facts This Design Rests On

Each fact below was read from the cited source. Wasmtime facts are from
`v49.0.0-rc.1`, the version that the polyfill pins.

- Wasmtime's macro accepts five invocation forms: `bindgen!()`,
  `bindgen!("world")`, `bindgen!(in "path")`, `bindgen!("world" in "path")`, and
  `bindgen!({ … })` ([Wasmtime bindgen docs]).
- The parser of the option map accepts 19 options ([Wasmtime bindgen options]).
- The `imports` and `exports` options give each function a set of six flags:
  `async`, `trappable`, `store`, `tracing`, `verbose_tracing`, and `exact`
  ([Wasmtime flags]).
- Only the first rule that matches a function applies. A rule that matches no
  function fails the compile ([Wasmtime flag rules], [Wasmtime unused rules]).
- A rule can name the destructor of a resource `r` as `[drop]r` ([Wasmtime flag
  rules]).
- An async function always gets the `async` and `store` flags ([Wasmtime async
  flags]).
- For a plain import, the `async` flag registers the function with
  `func_wrap_async`. The host future holds the store until it completes
  ([Wasmtime concurrent expansion]).
- A function with the `store` flag is a method of a trait named
  `HostWithStore<T>`. A resource `r` has a trait `HostRWithStore<T>` for the
  same purpose ([Wasmtime import traits], [Wasmtime resource traits]).
- An async import takes `&Accessor<T, Self>` and returns a future. Its
  registration uses `func_wrap_concurrent` ([Wasmtime import traits]).
- For an export, the `async` flag makes `call_*` an `async fn` over
  `call_async`. The `async` and `store` flags together make `call_*` take an
  accessor and use `call_concurrent` ([Wasmtime async expansion], [Wasmtime
  concurrent export]). The `store` flag alone changes nothing on an export.
- An export takes borrowed parameters, such as `&str`, except under
  `async | store`, where it takes owned parameters ([Wasmtime parameter mode]).
- Wasmtime's own bindings for WASI 0.3 set the `store` flag on plain functions
  that create a stream or a future. They set `async | store` on every export.
  They also set the `async` flag on five plain socket functions ([Wasmtime WASI
  bindings], [Wasmtime WASI HTTP bindings]).
- A `with` key names an interface, a package, a namespace, a version of a
  package, or one type. Lookup goes from the most specific key to the least
  specific key ([Wasmtime with lookup]). A `with` key that matches nothing fails
  the compile ([Wasmtime with unused]).
- A resource that `with` maps becomes `pub use path as Name`. Any other imported
  resource becomes the empty enum `pub enum Name {}` ([Wasmtime with resource]).
- The `trappable` flag makes a method return `wasmtime::Result<T>`. An `Err`
  traps the guest ([Wasmtime trappable], [Wasmtime trappable closure]).
- `trappable_error_type` acts only on a function with the `trappable` flag. It
  makes a function that returns `result<T, E>` return `Result<T, MyError>`. The
  interface trait gains `convert_e`, whose `Ok` value reaches the guest and
  whose `Err` traps ([Wasmtime trappable error], [Wasmtime convert]).
- `add_to_linker` takes a projection `fn(&mut T) -> D::Data<'_>`. `D` implements
  `HasData`, a trait with one generic associated type. `HasSelf<T>` is the
  projection to `&mut T` ([Wasmtime has data]).
- `Access<'a, T, D>` holds a store context and a projection. `Accessor<T, D>`
  holds a store token and a projection. Its `with` method gives an `Access`.
  `AsAccessor` is the trait of any accessor, whatever its projection ([Wasmtime
  access]).
- `TypedFunc::call_concurrent` and the `close_with` and `guard` methods of the
  stream and future readers take an `impl AsAccessor` ([Wasmtime call
  concurrent], [Wasmtime readers]).
- The world struct has `instantiate`, `instantiate_async`, `new`, and
  `add_to_linker`. `{World}Pre` and `{World}Indices` rest on `InstancePre`
  ([Wasmtime world expansion]).
- The functions that a world imports outside an interface are methods of a trait
  named `{World}Imports` ([Wasmtime world imports]).
- With the `interfaces` option, Wasmtime's macro emits no world struct
  ([Wasmtime only interfaces]).
- A versioned interface becomes a module whose name carries the version, such as
  `dep0_1_0`. Its registration names the versioned interface ([Wasmtime
  versions]).
- Wasmtime's macro reads every WIT feature gate, as `all_features = true` sets
  ([Wasmtime features]). It emits `LinkOptions` to turn gated items on at link
  time ([Wasmtime link options]).
- Wasmtime's own WASI 0.3 WIT gates an import of the world `wasi:cli/command`
  behind `@unstable` ([Wasmtime WASI WIT]).
- The generator behind Wasmtime's macro is the crate
  `wasmtime-internal-wit-bindgen`. Wasmtime publishes it as an internal crate.
  It has about 4,900 lines and depends on `wit-parser` 0.258 ([Wasmtime
  generator]).
- The codegen corpus has 35 entries. The test suite expands each one under six
  sets of options ([Wasmtime codegen tests]). No entry declares an async
  function.
- The polyfill fails to link an async import that a plain registration
  satisfies, and a plain import that a concurrent registration satisfies. It
  gives Wasmtime's message ([PDD020]).
- `wasi:webgpu@0.3.0-rc.2` declares each operation that waits on the GPU as an
  async function. Each one is a method of a resource. It declares `lost` as a
  plain function that returns `future<gpu-device-lost-info>`, and
  `on-uncaptured-error` as a plain function that returns `stream<gpu-error>`. It
  has 33 resources and no feature gates ([wasi-webgpu]).

## The Macro

The polyfill exposes `bindgen!` at the root of the crate, as `wcmp::bindgen!`.
It accepts the five invocation forms of Wasmtime's macro. With no path, it reads
`wit/` beside the `Cargo.toml` of the crate that invokes it.

The macro runs at compile time on the build machine. It parses WIT with
`wit-parser`. The runtime crate does not parse WIT, and no browser binary
contains the parser.

The macro emits paths under `wcmp`. A crate that re-exports the polyfill tells
the macro where the polyfill is with the `wcmp_crate` option. The macro refuses
`wasmtime_crate` with a message that names `wcmp_crate`, as the derive of
[PDD027] does.

The macro is the polyfill's own generator. It is not a fork of
`wasmtime-internal-wit-bindgen`. That crate emits Wasmtime's shapes at almost
every point, and it has no stable API. A fork must change nearly every point and
then follow each release of Wasmtime. The polyfill's generator uses Wasmtime's
generator as the reference for structure and names.

### The Code for One Interface

The macro emits one module for each interface, at Wasmtime's path. For the
interface `example:app/log`, the module is `example::app::log`. A versioned
interface carries the version in its module name, as in Wasmtime, and its
registration names the versioned interface. An exported interface is under
`exports`. Each WIT type becomes a Rust type with the derives and the layout
assertions of [PDD027].

For each imported interface, the macro emits:

- A trait `Host` with one method for each plain function without the `store`
  flag.
- A trait `HostWithStore<T>` with one method for each function with the `store`
  flag. Each async function has the flag.
- For each resource `r`, a trait `HostR` and a trait `HostRWithStore<T>`. Their
  methods are the constructor, the methods, and the static functions of `r`,
  divided by the same rule. `HostR` also has `drop`.
- `add_to_linker_instance` and `add_to_linker`.

The functions and the resources that a world imports outside an interface get
the same traits, named `{World}Imports` and `{World}ImportsWithStore<T>`, and
the function `add_to_linker_imports`.

The host implements `Host` and `HostR` on the result of the projection. It
implements `HostWithStore<T>` and `HostRWithStore<T>` on the `HasData` type
itself, because their methods take no `self`.

This example shows an interface and the code that the macro emits for it:

```wit
package example:app;

interface log {
  enum level { info, error }
  log: func(level: level, message: string);
}
```

```rust
pub mod example {
    pub mod app {
        pub mod log {
            #[derive(wcmp::ComponentType, wcmp::Lift, wcmp::Lower)]
            #[component(enum)]
            #[repr(u8)]
            pub enum Level {
                #[component(name = "info")]
                Info,
                #[component(name = "error")]
                Error,
            }

            pub trait Host {
                fn log(&mut self, level: Level, message: String);
            }

            pub trait HostWithStore<T>: wcmp::HasData {}

            pub fn add_to_linker<T, D>(
                linker: &mut wcmp::Linker<T>,
                host_getter: fn(&mut T) -> D::Data<'_>,
            ) -> wcmp::Result<()>
            where
                D: HostWithStore<T>,
                for<'a> D::Data<'a>: Host,
                T: 'static;
        }
    }
}
```

`add_to_linker` reaches the linker instance of the interface and registers each
function. Each registration calls the projection on the host data, then calls
the trait method on the result.

### Imports

The flags and the kind of a function select its trait and its registration:

| Import                                  | Trait method                                | Registration           |
| --------------------------------------- | ------------------------------------------- | ---------------------- |
| plain function                          | `Host::f(&mut self, …) -> R`                | `func_wrap`            |
| plain function with `store`             | `HostWithStore::f(Access<T, Self>, …) -> R` | `func_wrap`            |
| async function                          | `HostWithStore::f(&Accessor<T, Self>, …)`   | `func_wrap_concurrent` |
| plain function with `async`             | none                                        | refused                |
| plain function with `async` and `store` | none                                        | refused                |

The method of an async function returns `impl Future<Output = R>`. Natively, the
future and the traits are `Send`. In the browser, they are not required to be
`Send`. The generated code expresses the bound with one marker trait whose
definition differs per target, as [PDD018] does for host futures. The macro
therefore emits the same tokens for both targets. A browser host awaits
JavaScript promises and holds JavaScript objects, and neither is `Send`. A
native host written for Wasmtime meets the stricter bound.

The registration of a plain function with the `store` flag builds the `Access`
from the `HostCall` of the call ([PDD011]). It needs no new kind of host
function.

The `async` flag on a plain import is refused. The refusal names the missing
host function, which holds the store across an await, and it suggests the
declaration `async func` in WIT. Every other import form in this table rests on
a call surface of [PDD011] or [PDD020].

### Exports

Every call into a component is an `async fn` in the polyfill ([PDD010]). The
macro therefore emits each `call_*` method as an `async fn` in every case:

```rust
impl Guest {
    pub async fn call_greet<T: 'static>(&self, store: &mut wcmp::Store<T>, name: &str) -> wcmp::Result<String>;
    pub fn func_greet(&self) -> wcmp::TypedFunc<(&str,), (String,)>;
}
```

Parameters follow Wasmtime's rule. A `call_*` method takes borrowed parameters,
such as `&str` and `&[T]`, except under `async | store`, where it takes owned
parameters. [PDD027] gives `str` and `[T]` a lowering.

The `async` flag on an export is accepted and changes nothing. Its absence is
not an error. An export with the `async` and `store` flags, and every async
export, takes an `impl AsAccessor` in place of the store and calls
`TypedFunc::call_concurrent`. The `store` flag alone changes nothing on an
export.

An exported resource is `ResourceAny`, as in Wasmtime. Its getter has
`call_constructor`, and one `call_*` method for each method and static function.

### The World

The macro emits one struct for the world, named after the world in upper camel
case. It has these items:

- `async fn instantiate_async(store: &mut Store<T>, component: &Component, linker: &Linker<T>) -> Result<World>`.
  It instantiates the component and then calls `new`.
- `fn new(store: &mut Store<T>, instance: &Instance) -> Result<World>`.
- `add_to_linker`, which calls `add_to_linker` of each imported interface and
  `add_to_linker_imports`.
- One `call_*` method for each function that the world exports outside an
  interface.
- One interface getter for each exported interface, such as
  `fn example_app_run(&self) -> &exports::example::app::run::Guest`.

`new` acquires each export as a `TypedFunc` and keeps it in the struct. It runs
the type check of each export once, at this point. A `call_*` method never runs
the type check. This follows the cost model of [PDD027]. An export whose type
does not match fails `new` with `Error::TypeMismatch` and the path of [PDD027].

This design adds `Error::MissingExport`. It holds the name of the export and the
name of the instance that does not have it. `new` fails with it when the
component does not export an interface or a function of the world.

`new` takes the store, as Wasmtime's `new` does. The lookups of this design do
not use it. The parameter keeps the glue modules alike.

The struct has no `instantiate`, because no call into a component is
synchronous. It has no `{World}Pre` and no `{World}Indices`, because the
polyfill has no pre-instantiated component.

With the `interfaces` option, the macro emits the interface modules and no world
struct, as Wasmtime's macro does.

## Host Data Projection

A host crate that serves an interface does not know the `T` of the embedder's
`Store<T>`. Wasmtime solves this with a projection, and the polyfill adopts the
same types:

```rust
pub trait HasData: 'static {
    type Data<'a>;
}

pub struct HasSelf<T: ?Sized>(PhantomData<T>);

impl<T: ?Sized + 'static> HasData for HasSelf<T> {
    type Data<'a> = &'a mut T;
}
```

No value of a `HasData` type exists. The type only names the result of the
projection. A host that implements the traits on its whole host data passes
`HasSelf<_>` and the identity function:

```rust
example::app::log::add_to_linker::<_, HasSelf<_>>(&mut linker, |state| state)?;
```

A host crate implements the traits on a view of its own state. A view can hold
several separate borrows of the host data:

```rust
pub struct GpuView<'a> {
    pub ctx: &'a mut GpuCtx,
    pub table: &'a mut ResourceTable,
}

struct HasGpu;

impl HasData for HasGpu {
    type Data<'a> = GpuView<'a>;
}
```

The projection captures nothing, and each registration keeps a copy of it.

### Access and Accessor

`Access<'a, T, D = HasSelf<T>>` is the short-lived view that a function with the
`store` flag receives. It holds the store context and the projection:

- `get` gives `D::Data<'_>`.
- `data` and `data_mut` give the host data.
- `store` gives the store context, as `HostCall::store` does ([PDD011]).

`Accessor<T, D = HasSelf<T>>` is the token of [PDD020] with a projection added:

- `with` runs a closure that receives an `Access<'_, T, D>`.
- `getter` gives the projection.
- `with_getter::<D2>` gives an accessor of the same store with another
  projection.

The generated registration of an async function uses `with_getter` to give the
trait method an `&Accessor<T, D>`.

`AsAccessor` is the trait of any accessor, whatever its projection. `Accessor`
and a reference to an `AsAccessor` implement it. `TypedFunc::call_concurrent`,
`Func::call_concurrent`, and the `close_with` and `guard` methods of the readers
take an `impl AsAccessor`, as in Wasmtime. A host method that holds an
`&Accessor<T, Self>` therefore calls each of them.

Two parts of the polyfill's accessor differ from Wasmtime's:

- `with` returns `Result<R>`. A reach from inside another reach, or outside a
  poll of the store, fails with an error of [PDD020]. Wasmtime panics. The
  polyfill returns structured errors at its public boundary ([PDD005]).
- `Accessor` is `Clone`. The polyfill's host futures are `'static`, so a future
  owns its own copy of the token.

## Resources and `with`

The `with` option follows Wasmtime at each granularity. A key names an
interface, a package, a namespace, a version of a package, or one type. The
macro looks up the most specific key first. A key that matches nothing fails the
compile. A path in `with` carries no generic parameters.

When `with` maps an interface or a package, the macro emits nothing for it. The
generated code uses the traits and the types of the mapped module. The mapped
module must come from the macro of the polyfill. A module from Wasmtime's macro
names Wasmtime's types, and the code does not compile.

When `with` maps a resource, the macro emits `pub use path as Name` in place of
the empty enum. The handle is then `Resource<Name>`, and its identity is
`ResourceType::host::<Name>()` ([PDD027]). `ResourceTable::push` returns a
`Resource<T>` for the value that it stores. A host that maps a resource to its
own type therefore pushes its value and returns the handle with no conversion:

```rust
wcmp::bindgen!({
    world: "imports",
    imports: { default: trappable },
    with: { "wasi:webgpu/webgpu.gpu-buffer": crate::core::Buffer },
});

// In `create_buffer`, which returns `wcmp::Result<Resource<crate::core::Buffer>>`:
let buffer = self.table.push(crate::core::Buffer::new(raw))?;
Ok(buffer)
```

The registration of an imported resource calls `LinkerInstance::resource` with
the name, `ResourceType::host::<Name>()`, and a destructor. The destructor calls
`drop` with a `Resource<Name>`, and `drop` returns `wcmp::Result<()>`.

`drop` is synchronous in every interface, also in an interface of async
functions. A rule for `[drop]r` with the `store` flag moves `drop` to
`HostRWithStore<T>`, where it receives an `Access`. A rule for `[drop]r` with
the `async` flag is refused.

An `eq` resource needs a registration only in the interface that defines it.
This design assumes Wasmtime's rule for this link.

## Errors

The generated code uses `wcmp::Result<T>` where Wasmtime's code uses
`wasmtime::Result<T>`:

- A method with the `trappable` flag returns `wcmp::Result<T>`.
- `convert_e` of `trappable_error_type` returns `wcmp::Result<E>`.
- `drop` returns `wcmp::Result<()>`.

A method without the `trappable` flag returns `T`, as in Wasmtime.

A host reports an error of its own with `Error::Host`. The variant holds an
`anyhow::Error` as its source, and `Error` implements `From<anyhow::Error>`. A
host method therefore uses `?` on its own errors, as on Wasmtime:

```rust
use anyhow::Context;

fn read(&mut self, path: String) -> wcmp::Result<Vec<u8>> {
    let bytes = std::fs::read(&path).with_context(|| format!("read {path}"))?;
    Ok(bytes)
}
```

A host error travels this way:

1. The method returns `Err`.
2. The guest traps, and the trap poisons the store ([PDD023]).
3. The host's `call_*` returns the same `Error`. An `Error::Host` keeps its
   source chain, so the host can downcast the source. Any other variant that the
   method passed on with `?` comes back unchanged.

`convert_e` follows Wasmtime. An `Ok(e)` reaches the guest as the WIT error
value. An `Err` traps.

With `anyhow: true`, the generated signatures use `anyhow::Result<T>`. The
registration turns an `Err` into an `Error`. When the `anyhow::Error` holds an
`Error` of the polyfill, the registration takes that `Error` out. Any other
`anyhow::Error` becomes the source of an `Error::Host`.

In the browser, a JavaScript exception is a `JsValue`, which is not `Send`. A
browser host turns it into a message before it wraps it in an `anyhow::Error`.

## The Option Ledger

Each option and each flag of Wasmtime's macro has one entry. Matched means the
same meaning as in Wasmtime. Each refusal names the option and the reason.

| Option                                                   | Entry                                      |
| -------------------------------------------------------- | ------------------------------------------ |
| `world`, `path`, `inline`, `interfaces`                  | matched                                    |
| `with`                                                   | matched                                    |
| `imports`, `exports`                                     | matched, with the flags below              |
| `trappable_error_type`                                   | matched                                    |
| `additional_derives`, `skip_mut_forwarding_impls`        | matched                                    |
| `stringify`, `debug`, `include_generated_code_from_file` | matched                                    |
| `anyhow`                                                 | matched                                    |
| `require_store_data_send`                                | matched natively, no effect in the browser |
| `ownership: Owning`                                      | matched                                    |
| `ownership: Borrowing { … }`                             | refused                                    |
| `named_imports`                                          | refused                                    |
| `include_component_type`                                 | refused                                    |
| `wasmtime_crate`                                         | refused, the message names `wcmp_crate`    |

| Flag                                    | Imports | Exports                         |
| --------------------------------------- | ------- | ------------------------------- |
| `trappable`, `exact`                    | matched | matched                         |
| `tracing`, `verbose_tracing`            | matched | matched                         |
| `store` on a plain function             | matched | no effect alone                 |
| `async` on a plain function             | refused | accepted, no effect             |
| `async` and `store` on a plain function | refused | matched, uses `call_concurrent` |
| `store` on `[drop]r`                    | matched | not applicable                  |
| `async` on `[drop]r`                    | refused | not applicable                  |

The polyfill adds one option of its own, `wcmp_crate`.

The macro reads every WIT feature gate, as Wasmtime's macro does. It generates
each item behind `@unstable` as if every feature is on. A gated import gets its
trait method and its registration. A component that does not import it leaves
the registration unused. This is the "tolerate" target of [PDD003]. The macro
generates no `LinkOptions`.

### Keeping Up With Wasmtime

Two tests hold the macro to Wasmtime's option set:

- The ledger test reads the options and the flags that the parser of Wasmtime's
  macro accepts at the pinned version. It compares them with the option ledger.
  An option or a flag without an entry fails the test.
- The codegen corpus test runs the macro over each entry of the codegen corpus
  at the pinned version.

The codegen corpus test uses Wasmtime's six sets of options, with these expected
results:

| Set                                           | Expected result                    |
| --------------------------------------------- | ---------------------------------- |
| no options                                    | compiles                           |
| `imports` and `exports` with `async`          | refused, `async` on a plain import |
| `imports` and `exports` with `async \| store` | refused, `async` on a plain import |
| `tracing \| verbose_tracing` and `Borrowing`  | refused, `Borrowing`               |
| `imports` with `store`                        | compiles                           |
| `anyhow: true` and `imports` with `trappable` | compiles                           |

The test adds three sets of the polyfill's own, so that each matched flag
compiles at least once:

- `imports` and `exports` with `tracing | verbose_tracing`, and no `Borrowing`.
- `exports` with `async | store`.
- `imports` and `exports` with `trappable | exact` on one named function.

An entry that needs a refused option fails with that refusal in every set.

When the pin of Wasmtime moves, a new option, a new flag, or a new corpus entry
fails one of the tests. The test passes after a person classifies it in the
option ledger.

## Both Targets

The macro emits the same code for every target. A host that runs on both
runtimes has this shape:

```text
host core:       the logic, with no runtime types
Wasmtime glue:   wasmtime::component::bindgen!, trait impls that call the core
polyfill glue:   wcmp::bindgen!, trait impls that call the core

a compile-time condition of the host selects one glue module
```

The polyfill glue module compiles natively and in the browser. A host that runs
natively on Wasmtime and in the browser on the polyfill selects by target. A
host that runs on the polyfill everywhere needs no Wasmtime glue module.

The two glue modules differ only where this design lists a difference from
Wasmtime:

- The crate path of each name.
- The invocation of Wasmtime's macro sets `exports: { default: async }` or
  `async | store`. Without it, Wasmtime's `call_*` is synchronous and the calls
  differ by `.await`.
- An invocation of Wasmtime's macro that sets the `async` flag on a plain import
  drops it for the polyfill. The polyfill glue module serves that function
  synchronously.
- `call_*` takes `S: AsContextMut` in Wasmtime and `&mut Store<T>` in the
  polyfill. A call that passes `&mut store` is the same in both.
- A `?` at each `Accessor::with`.
- `Access::store` in place of `AsContextMut` on an `Access`.
- The Wasmtime glue module passes `LinkOptions` when its WIT has gated items.

## User Stories

A developer gives a component WebGPU in the browser.

> The developer writes a host core that wraps the browser's `GPUDevice` and its
> objects. The developer invokes `wcmp::bindgen!` on `wasi:webgpu` and maps each
> resource with `with` to a type of the core. The glue module implements
> `request-adapter` and `map-async` as async functions that await the browser's
> promises. It implements `lost` with the `store` flag and returns a
> `FutureReader`. A guest that renders through `wasi:webgpu` runs in Chrome.

The developer then runs the same guest natively.

> The developer adds a second glue module over Wasmtime's macro and a native GPU
> library, with the same host core. A compile-time condition selects the glue
> module. The second glue module copies the first, with the differences that
> this design lists.

A developer moves a Wasmtime host to the browser.

> The developer's host uses Wasmtime's macro with `trappable` imports, async
> exports, and a `with` mapping for its resources. The developer copies the
> invocation into a new glue module and changes `wasmtime` to `wcmp`. The
> invocation compiles. The trait impls compile after the changes that this
> design lists.

A developer uses an option that the polyfill does not support.

> The developer copies an invocation with `imports: { default: async }`. The
> compile fails. The error names the flag. It states that the polyfill has no
> host function that holds the store across an await. It suggests `async func`
> in the WIT.

A maintainer moves the pin of Wasmtime.

> The new release adds an option to Wasmtime's macro. The ledger test fails and
> names the option. The maintainer classifies it in the option ledger, and the
> test passes.

## Test Cases

The same host core runs under both macros. A test world has these items:

- A record, a variant, and a flags type.
- An imported resource that `with` maps.
- A `trappable` import and a `trappable` import with `trappable_error_type`.
- An async import.
- A plain import with the `store` flag that returns a future.
- An exported resource.

One host core serves it through a Wasmtime glue module natively and through a
polyfill glue module on both targets. One guest component runs under each of the
three. Each run records each value that crosses and each error. The records of
the three runs are equal. The test states the line count of each glue module.

A record, a variant, and a flags type cross as generated Rust types. A generated
export takes a record and returns a variant. A generated import takes a variant
and a flags value and returns a record. Each value that crosses equals the value
that the test expects, on both targets.

A resource crosses through `Resource<T>`. A generated constructor of an imported
resource pushes a host value into a `ResourceTable` and returns its handle. A
generated method receives the guest's `borrow` of it, and the rep matches. When
the guest drops the handle, `drop` runs once.

An exported resource crosses through `ResourceAny`. The host calls the generated
`call_constructor` of an exported resource and a generated `call_*` method on
the handle. The method returns the value that the guest computes from the
constructor's argument.

`with` maps a resource to an existing type. The test maps a resource to a type
of the host core. The generated trait methods take and return
`Resource<CoreType>`. A `with` key that matches no WIT item fails the compile.

`with` maps an interface. A second invocation maps an interface to the module
that a first invocation generated. The second invocation emits no module for it,
and its world links with the first module's traits.

A host error from a generated method traps the guest. A `trappable` method
returns `Error::Host` with a source of a host type. The guest traps. The host's
`call_*` returns an error whose source downcasts to the host type. The store is
poisoned.

`anyhow: true` keeps the error. A `trappable` method under `anyhow: true`
returns an `anyhow::Error` of a host type, and a second method returns an
`anyhow::Error` that holds `Error::ResourceTable`. The first call returns
`Error::Host` whose source downcasts to the host type. The second returns
`Error::ResourceTable`.

`trappable_error_type` converts a host error. `convert_e` returns `Ok` for one
host error, and the guest receives the WIT error value. It returns `Err` for
another host error, and the guest traps.

An async import awaits a host future. The method of an async function awaits a
future that is pending once. The guest receives the result. In the browser, the
future awaits a JavaScript promise and holds a value that is not `Send`.

A plain import with the `store` flag creates a future. A generated method with
the shape of `wasi:webgpu`'s `lost` receives an `Access`, creates a
`FutureReader` through the store context, and returns it. The guest reads the
value of the future.

An export runs as `async | store`. An invocation with
`exports: { default: async | store }` emits `call_*` methods that take an
`impl AsAccessor`. A call from inside `run_concurrent` with an accessor of a
projection other than `HasSelf` returns the export's result.

The projection reaches a view. A host crate implements the generated traits on a
view type with two borrows of the host data, and registers with its own
`HasData` type. A call reaches both borrows. An async method of the same crate
calls `with_getter` and reaches the view through the new accessor.

The world runs the type check once. `new` on a component whose export has a
different record fails with `Error::TypeMismatch` and the path of the wrong
field. `new` on a component without one of the world's exports fails with
`Error::MissingExport` that names it. A test counts the runs of the type check
around 1,000 calls of one `call_*` method. The count does not change.

The invocation forms read WIT. A test invokes the macro in each of the five
forms, and with `inline` and with `interfaces`. Each one generates the expected
modules. The form with `interfaces` emits no world struct.

The macro refuses each unsupported option. A compile-fail test covers each
refusal of the option ledger. Each error names the option and the reason. The
error for the `async` flag on a plain import suggests `async func`.

The codegen corpus gives the expected results. Under each set of the codegen
corpus test, each entry compiles or fails with the refusal that the table of
expected results gives. Both targets build each entry that compiles.

The ledger is complete. The ledger test finds an entry for each option and each
flag that the parser of Wasmtime's macro accepts at the pinned version.

A gated item generates. A WIT world with an `@unstable` import generates its
trait method and its registration. A component that does not import it links.

`wasi:webgpu` generates. A pinned copy of the WIT of `wasi:webgpu@0.3.0-rc.2`
generates with a `with` mapping for its resources. The invocation sets the
`store` flag on `lost` and on `on-uncaptured-error`. The output compiles on both
targets.

The output keeps its shape. Snapshot tests through `stringify` record the output
of the macro for a set of corpus entries. The output of each one matches its
committed snapshot.

## Revisions to Earlier Designs

[PDD020] is revised in place. `Accessor` carries a projection, `with` gives an
`Access`, the calls that take an accessor take an `impl AsAccessor`, and the
polyfill has `HasData` and `HasSelf`.

[PDD021] is revised in place. `close_with` and `guard` take an
`impl AsAccessor`.

[PDD008] and [PDD005] are revised in place. A host reports its own error with
`Error::Host`, and the host can downcast its source.

[PDD003] is revised in place. It lists the host data projection as "Implement".

## References

- [PDD003], the compatibility outlook, revised in place by this design.
- [PDD005], the library foundations, revised in place by this design.
- [PDD007], linking and instantiation.
- [PDD008], the Canonical ABI and host functions, revised in place by this
  design.
- [PDD009], resources.
- [PDD010], the public API before WASI 0.3.
- [PDD011], host registration and the store context.
- [PDD018], the concurrency runtime model.
- [PDD020], subtasks and the async import, revised in place by this design.
- [PDD021], streams and futures, revised in place by this design.
- [PDD023], cancellation and the poisoned store.
- [PDD027], typed component values.
- [Wasmtime] at `v49.0.0-rc.1`. These are the parts that this design cites:
  - The macro: its [documentation][Wasmtime bindgen docs], its
    [options][Wasmtime bindgen options], its [feature gates][Wasmtime features],
    and its [generator][Wasmtime generator].
  - The flags: the [list][Wasmtime flags], the [rules][Wasmtime flag rules],
    [unused rules][Wasmtime unused rules], and [async
    functions][Wasmtime async flags].
  - The expansions: [async][Wasmtime async expansion],
    [concurrent][Wasmtime concurrent expansion], a [concurrent
    export][Wasmtime concurrent export], the [world][Wasmtime world expansion],
    [world imports][Wasmtime world imports], and [versions][Wasmtime versions].
  - The generated traits: [imports][Wasmtime import traits],
    [resources][Wasmtime resource traits], the [parameter
    mode][Wasmtime parameter mode], and the [world
    struct][Wasmtime only interfaces].
  - `with`: [lookup][Wasmtime with lookup], [unused keys][Wasmtime with unused],
    and [mapped resources][Wasmtime with resource].
  - Errors: [`trappable`][Wasmtime trappable], its
    [closure][Wasmtime trappable closure], [trappable
    errors][Wasmtime trappable error], and [`convert`][Wasmtime convert].
  - The projection: [`HasData`][Wasmtime has data], [`Access` and
    `Accessor`][Wasmtime access], [`call_concurrent`][Wasmtime call concurrent],
    and the [readers][Wasmtime readers].
  - [Link options][Wasmtime link options] and the [codegen
    tests][Wasmtime codegen tests].
  - The bindings and the WIT of WASI 0.3: [WASI][Wasmtime WASI bindings], [WASI
    HTTP][Wasmtime WASI HTTP bindings], and [`wasi:cli`][Wasmtime WASI WIT].
- [wasi-webgpu] at commit `6a776ba`.

[PDD003]: ./PDD003%20Compatibility%20Outlook.md
[PDD005]: ./PDD005%20Library%20Foundations.md
[PDD007]: ./PDD007%20Linking%20and%20Instantiation.md
[PDD008]: ./PDD008%20Canonical%20ABI%20and%20Host%20Functions.md
[PDD009]: ./PDD009%20Resources.md
[PDD010]: ./PDD010%20Pre-wasip3%20Public%20API.md
[PDD011]:
  ./PDD011%20Host%20Registration:%20Root%20Namespace%20and%20Store%20Context.md
[PDD018]: ./PDD018%20Concurrency%20Runtime%20Model.md
[PDD020]: ./PDD020%20Subtasks%20and%20the%20Async%20Import.md
[PDD021]: ./PDD021%20Streams%20and%20Futures.md
[PDD023]:
  ./PDD023%20Cancellation,%20Error%20Contexts,%20and%20the%20Poisoned%20Store.md
[PDD027]: ./PDD027%20Typed%20Component%20Values.md
[Wasmtime]: https://github.com/bytecodealliance/wasmtime
[Wasmtime bindgen docs]:
  https://github.com/bytecodealliance/wasmtime/blob/v49.0.0-rc.1/crates/wasmtime/src/runtime/component/mod.rs#L177-L540
[Wasmtime bindgen options]:
  https://github.com/bytecodealliance/wasmtime/blob/v49.0.0-rc.1/crates/component-macro/src/bindgen.rs#L276-L296
[Wasmtime features]:
  https://github.com/bytecodealliance/wasmtime/blob/v49.0.0-rc.1/crates/component-macro/src/bindgen.rs#L196-L200
[Wasmtime generator]:
  https://github.com/bytecodealliance/wasmtime/tree/v49.0.0-rc.1/crates/wit-bindgen
[Wasmtime flags]:
  https://github.com/bytecodealliance/wasmtime/blob/v49.0.0-rc.1/crates/wit-bindgen/src/config.rs#L5-L15
[Wasmtime flag rules]:
  https://github.com/bytecodealliance/wasmtime/blob/v49.0.0-rc.1/crates/wit-bindgen/src/config.rs#L95-L159
[Wasmtime unused rules]:
  https://github.com/bytecodealliance/wasmtime/blob/v49.0.0-rc.1/crates/wit-bindgen/src/lib.rs#L1148-L1149
[Wasmtime async flags]:
  https://github.com/bytecodealliance/wasmtime/blob/v49.0.0-rc.1/crates/wit-bindgen/src/config.rs#L66-L93
[Wasmtime async expansion]:
  https://github.com/bytecodealliance/wasmtime/blob/v49.0.0-rc.1/crates/component-macro/tests/expanded/char_async.rs#L360-L384
[Wasmtime concurrent expansion]:
  https://github.com/bytecodealliance/wasmtime/blob/v49.0.0-rc.1/crates/component-macro/tests/expanded/smoke_concurrent.rs#L181-L209
[Wasmtime concurrent export]:
  https://github.com/bytecodealliance/wasmtime/blob/v49.0.0-rc.1/crates/component-macro/tests/expanded/char_concurrent.rs#L354-L389
[Wasmtime world expansion]:
  https://github.com/bytecodealliance/wasmtime/blob/v49.0.0-rc.1/crates/component-macro/tests/expanded/smoke.rs#L9-L175
[Wasmtime world imports]:
  https://github.com/bytecodealliance/wasmtime/blob/v49.0.0-rc.1/crates/component-macro/tests/expanded/direct-import.rs#L101-L200
[Wasmtime versions]:
  https://github.com/bytecodealliance/wasmtime/blob/v49.0.0-rc.1/crates/component-macro/tests/expanded/multiversion.rs#L197-L300
[Wasmtime import traits]:
  https://github.com/bytecodealliance/wasmtime/blob/v49.0.0-rc.1/crates/wit-bindgen/src/lib.rs#L2978-L2987
[Wasmtime resource traits]:
  https://github.com/bytecodealliance/wasmtime/blob/v49.0.0-rc.1/crates/wit-bindgen/src/lib.rs#L3293-L3315
[Wasmtime parameter mode]:
  https://github.com/bytecodealliance/wasmtime/blob/v49.0.0-rc.1/crates/wit-bindgen/src/lib.rs#L3075-L3079
[Wasmtime only interfaces]:
  https://github.com/bytecodealliance/wasmtime/blob/v49.0.0-rc.1/crates/wit-bindgen/src/lib.rs#L1144-L1146
[Wasmtime with lookup]:
  https://github.com/bytecodealliance/wasmtime/blob/v49.0.0-rc.1/crates/wit-bindgen/src/lib.rs#L1251-L1360
[Wasmtime with unused]:
  https://github.com/bytecodealliance/wasmtime/blob/v49.0.0-rc.1/crates/wit-bindgen/src/lib.rs#L1128-L1142
[Wasmtime with resource]:
  https://github.com/bytecodealliance/wasmtime/blob/v49.0.0-rc.1/crates/wit-bindgen/src/lib.rs#L1862-L1873
[Wasmtime trappable]:
  https://github.com/bytecodealliance/wasmtime/blob/v49.0.0-rc.1/crates/wit-bindgen/src/lib.rs#L3029-L3052
[Wasmtime trappable closure]:
  https://github.com/bytecodealliance/wasmtime/blob/v49.0.0-rc.1/crates/wit-bindgen/src/lib.rs#L2958-L2965
[Wasmtime trappable error]:
  https://github.com/bytecodealliance/wasmtime/blob/v49.0.0-rc.1/crates/wit-bindgen/src/lib.rs#L2932-L2957
[Wasmtime convert]:
  https://github.com/bytecodealliance/wasmtime/blob/v49.0.0-rc.1/crates/wit-bindgen/src/lib.rs#L3407-L3420
[Wasmtime has data]:
  https://github.com/bytecodealliance/wasmtime/blob/v49.0.0-rc.1/crates/wasmtime/src/runtime/component/has_data.rs#L288-L305
[Wasmtime access]:
  https://github.com/bytecodealliance/wasmtime/blob/v49.0.0-rc.1/crates/wasmtime/src/runtime/component/concurrent.rs#L216-L520
[Wasmtime call concurrent]:
  https://github.com/bytecodealliance/wasmtime/blob/v49.0.0-rc.1/crates/wasmtime/src/runtime/component/concurrent/func.rs#L343-L351
[Wasmtime readers]:
  https://github.com/bytecodealliance/wasmtime/blob/v49.0.0-rc.1/crates/wasmtime/src/runtime/component/concurrent/futures_and_streams.rs#L1428-L1440
[Wasmtime link options]:
  https://github.com/bytecodealliance/wasmtime/blob/v49.0.0-rc.1/crates/wit-bindgen/src/lib.rs#L3589-L3718
[Wasmtime codegen tests]:
  https://github.com/bytecodealliance/wasmtime/blob/v49.0.0-rc.1/crates/component-macro/tests/codegen.rs#L3-L47
[Wasmtime WASI bindings]:
  https://github.com/bytecodealliance/wasmtime/blob/v49.0.0-rc.1/crates/wasi/src/p3/bindings.rs#L76-L97
[Wasmtime WASI HTTP bindings]:
  https://github.com/bytecodealliance/wasmtime/blob/v49.0.0-rc.1/crates/wasi-http/src/p3/bindings.rs#L9-L18
[Wasmtime WASI WIT]:
  https://github.com/bytecodealliance/wasmtime/blob/v49.0.0-rc.1/crates/wasi/src/p3/wit/deps/cli.wit#L207-L208
[wasi-webgpu]:
  https://github.com/WebAssembly/wasi-webgpu/blob/6a776bada0b66d3dbf9da304a49ff2947ce4e1f8/wit/webgpu.wit
