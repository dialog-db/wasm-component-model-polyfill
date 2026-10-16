# Typed Component Values

A host of the polyfill passes values to a component and receives values from it.
The host can write each value as a `Val` tree, or it can use plain Rust types.
This document designs the second form for every value type of the Component
Model. A WIT record becomes a Rust struct. A variant or an enum becomes a Rust
enum. A resource becomes a typed handle.

The design follows Wasmtime's `wasmtime::component` API for the definition of
typed values, resource handles, and the table of host resources. In that code, a
host author writes the same source for the polyfill as for Wasmtime. The `use`
lines name `wcmp` in place of `wasmtime`, and nothing else changes. A binding
generator can therefore emit one shape of type definitions for both.

The call surfaces keep the shapes that earlier designs gave them. Three of them
differ from Wasmtime:

- `Func::typed` takes no store ([PDD010]).
- A call through `TypedFunc` is an `async fn` on every target ([PDD010]).
- A host function's closure receives `HostCall` where Wasmtime gives
  `StoreContextMut` ([PDD011]).

Fifteen terms recur:

- A typed value is a Rust value whose type implements the three traits of this
  design.
- The typed path is every call surface that takes typed values: `TypedFunc`,
  `func_wrap`, and `func_wrap_concurrent`.
- The dynamic path is every call surface that takes `Val`: `func_new`,
  `Func::call`, and `Func::call_concurrent`.
- To lower is to write a host value into a guest's memory or flat values.
  [PDD008] defines it.
- To lift is to read a guest value into a host value. [PDD008] defines it.
- A crossing is one lift or one lower of one value.
- Flat values are the core Wasm values that the Canonical ABI uses for arguments
  and results before it spills them to memory.
- The type check compares a Rust type with a type that a component declares.
- A derived type is a Rust type whose traits a derive macro writes.
- A host resource is a resource type that the host defines.
- A guest resource is a resource type that a component defines.
- The identity of a resource type is the value that tells two resource types
  apart.
- A rep is the `u32` that represents one resource to its definer. [PDD009]
  defines it.
- The host table is the table in the store that holds the handles the host owns
  or lends. It is internal to the polyfill.
- Generated code is the Rust code that a binding generator emits from WIT.

## Goals

- A host author defines and passes typed values with Wasmtime's names and
  shapes. These include `ComponentType`, `Lift`, `Lower`, the derive macros,
  `flags!`, `Resource<T>`, `ResourceDynamic`, `ResourceAny`, and
  `ResourceTable`.
- Every value type of the Component Model has a Rust form on the typed path.
- No crossing on the typed path builds a `Val`. A derived type lifts and lowers
  directly, field by field.
- The type check compares structure and names, as Wasmtime does. It compares
  resource types by identity, never by name.
- The type check runs when the host acquires a typed function or links a host
  function. It never runs on a call.
- A type mismatch fails with `Error::TypeMismatch` and a position that holds the
  path to the wrong part.
- A resource type's identity is one value, `ResourceType`, on every surface of
  the polyfill.
- The derive and its output compile and behave the same natively and on
  `wasm32-unknown-unknown`.
- The benchmark suite measures each cost that this design names, on the web lane
  first.
- Generated code has a fixed contract to target. This document lists it.

## Non-goals

- A binding generator. This design sets the contract that generated code uses,
  and it designs no generator.
- A borrowed lift of a string or a list, such as Wasmtime's `WasmStr` and
  `WasmList`. They save no copy in the browser. Natively they need a borrow of
  guest memory that lasts past one closure, and the runtime layer of [PDD025]
  lends memory only inside a closure.
- A pre-instantiated component, such as Wasmtime's `InstancePre`. The type check
  of a host function runs once for each link, as it does in Wasmtime's
  `Linker::instantiate`.
- A change to the call surfaces of [PDD010] and [PDD011].
- The conversion of a host error into a trap or a WIT error value, beyond the
  design of [PDD008].
- Asynchronous resource destructors, such as Wasmtime's `resource_async` and
  `resource_concurrent`.
- The `bytes` crate's `Bytes` as a list, and `Rc` as a forwarding type. Wasmtime
  gates the first behind a feature and excludes the second.
- Aliases for the names that this design replaces.
- The rule that links a resource that one interface uses from another. This
  design assumes Wasmtime's rule: the defining interface's registration
  satisfies the `eq` import of every other interface.

## Facts This Design Rests On

Each fact below was read from the cited source. Wasmtime facts are from
`v49.0.0-rc.1`, the version that the polyfill pins.

- Wasmtime's three traits are `ComponentType`, `Lower`, and `Lift` ([Wasmtime
  traits]). Each method and each constant is `#[doc(hidden)]`. The documentation
  tells users to derive the traits, not to implement them.
- `ComponentType` carries the Canonical ABI layout of the type as constants:
  `ABI`, `SIZE32`, `ALIGN32`, and the count of flat values ([Wasmtime traits]).
- Wasmtime's typed path never builds a `Val`. `Val` does not implement
  `ComponentType` ([Wasmtime values]).
- Wasmtime checks a record by its field count, then field by field: the type,
  then the name. It checks a variant by its case count, then case by case: the
  name, then the payload. It checks an enum or a flags type by count, then by
  each name in order ([Wasmtime type check]).
- `Func::typed` runs the type check of an export ([Wasmtime typed]). The type
  check of a host function runs when the linker meets a component, in
  `instantiate_pre`. It does not run on a call ([Wasmtime linker], [Wasmtime
  matching]).
- The derive accepts `#[component(record)]`, `#[component(variant)]`,
  `#[component(enum)]`, `#[component(name = "…")]`, and
  `#[component(wasmtime_crate = …)]` ([Wasmtime derive]). An enum must carry
  `#[repr(u8)]`, `#[repr(u16)]`, or `#[repr(u32)]`. Without a `name` attribute,
  the WIT name is the Rust name as written.
- Generated code asserts the layout of each derived type through `SIZE32` and
  `ALIGN32` ([Wasmtime bindgen]).
- Wasmtime's `ResourceType` is the identity of a resource type.
  `ResourceType::host::<T>()` keys it on the Rust type `T`.
  `ResourceType::host_dynamic` keys it on a `u32` ([Wasmtime resource type]).
  Wasmtime's type reflection carries the same `ResourceType` in its `own` and
  `borrow` types ([Wasmtime types]).
- A registration takes a name, a `ResourceType`, and a destructor that receives
  the store and the rep ([Wasmtime resource registration]).
- `Resource<T>` has four states ([Wasmtime resource states]). It is a borrow, an
  owned handle in no table, an owned handle that moved into a guest, or an index
  into the host table. A second lowering of an owned handle as an `own` fails.
  The message is "host resource already consumed" ([Wasmtime consumed]).
- `ResourceDynamic` is the handle for a type from `host_dynamic`. It carries the
  rep and the `u32` of its type ([Wasmtime resource dynamic]).
- `ResourceAny` is a handle to a resource of any type. Its type check accepts
  any `own` or `borrow`, and its lowering compares the resource type ([Wasmtime
  resource any]). `Val` holds it as `Val::Resource` ([Wasmtime values]).
- A stale index into the host table fails with "host-owned resource was already
  de-allocated" ([Wasmtime stale]).
- `ResourceTable` lives in `wasmtime::component`, not in WASI. Its `push` and
  `push_child` require `T: Send + 'static` ([Wasmtime resource table]).
- Wasmtime accepts an `eq` resource import with no registration when an earlier
  import defined the resource ([Wasmtime matching]).
- In the browser, the runtime layer copies a range of guest memory once into the
  polyfill's memory before the polyfill reads it ([PDD025]). Rust on `wasm32`
  addresses only its own memory.

## The Three Traits

The polyfill exposes three traits with Wasmtime's names, at the root of the
crate:

- `ComponentType` describes a Rust type as a component value type. It carries
  the layout and the type check.
- `Lower` writes a value of the type into a guest.
- `Lift` reads a value of the type out of a guest.

Every method and every constant of the three traits is `#[doc(hidden)]`. A host
author derives the traits or uses a type that implements them. The hidden items
are the polyfill's own. They follow the Canonical ABI of [PDD008], not
Wasmtime's internal signatures.

The three traits require `Send + Sync` on every target, as Wasmtime's do. A
typed value is data. A JavaScript object in the browser is not data that crosses
the boundary. A host keeps it in a `ResourceTable` and passes a `Resource<T>`,
as the section on `ResourceTable` describes.

### Parameters and Results

A tuple of typed values implements `ComponentNamedList`, as in Wasmtime. The
tuple is the parameter list or the result list of a function. Tuples of 0 to 17
values implement it.

A typed function takes both of its lists as tuples. In this example, `length` is
the `Func` of an export that takes a `point` and returns a `u32`:

```rust
let length: TypedFunc<(Point,), (u32,)> = length.typed()?;
let (value,) = length.call(&mut store, (Point { x: 3, y: 4 },)).await?;
```

A host function receives its parameters as one tuple and returns its results as
one tuple:

```rust
linker.root().func_wrap("log", |_call, (level, message): (Level, String)| {
    println!("{level:?}: {message}");
    Ok(())
})?;
```

A function with one result returns a tuple of one value. This matches Wasmtime
and lets generated code treat every function the same way.

### Layout

Each type that implements `ComponentType` states its Canonical ABI layout as
constants: its size, its alignment, and its count of flat values. The derive
computes the constants of a derived type from the constants of its fields, at
compile time. `SIZE32` and `ALIGN32` are public constants. Generated code
asserts them, as Wasmtime's generated code does.

### The Lift and Lower Context

The hidden methods take a lift context or a lower context. Each context is a
public type in a hidden module, so that a derive in another crate can name it.
The context gives a method access to the guest's memory, its `realloc`, the
string encoding of the call, and the copy budget of [PDD021].

A context reads each contiguous range of guest memory once. The elements of a
list come from one read of the list's range. Each string or list that an element
points to is one more read. A context writes a lowered list once, after the host
builds its bytes. It calls `realloc` in the order that the Canonical ABI gives.
In the browser, each read and each write is one call into the JavaScript engine.

## Crossing Without Val

Every crossing on the typed path lifts and lowers directly. The typed path
builds no `Val` for a parameter, a result, a field, an element, or a case. This
covers:

- `TypedFunc::call` and `TypedFunc::call_concurrent`.
- `func_wrap` and `func_wrap_concurrent`, in both directions.
- The result of an export that the component lifts as `async`. The guest
  delivers that result through `task.return`, and the typed path lifts it
  directly from the arguments of `task.return`.

A list of a numeric type has a fast path: one write of the list's bytes and one
read. A list of any other type lifts and lowers each element through the element
type's own methods. A list of derived records therefore builds no `Val` at any
depth.

`Val` belongs to the dynamic path only. A host that uses `func_new` or
`Func::call` writes `Val` trees.

### What a Val Costs

The benchmark suite has a web lane and a native lane. The web lane runs the
suite in a browser. The native lane runs it as a native program. The suite
measured both crossings. Each number below is the median time of one call in
microseconds. The range after it is the 10th to the 90th percentile of the
samples.

The measurements were taken on 2026-10-15, at commit `97dc4609d`, with this
setup:

- The machine was an AMD Ryzen AI MAX+ 395 with 32 threads and 94 GiB of memory.
  No other heavy work ran on it.
- The build profile was `release`.
- The web lane ran in headless Chromium 154.0.8037.57, on the browser backend.
- The native lane ran on the Wasmtime backend.
- The controls were 16 warm-up iterations, 25 samples, and 50 ms for each timed
  batch. No batch reached its maximum size.

Each benchmark calls an `echo` component that returns the list it receives. The
record is `record point { x: u32, y: u32 }`.

| Benchmark                      | Crossing | Web                       | Native                    |
| ------------------------------ | -------- | ------------------------- | ------------------------- |
| `list<u8>`, 65,536 elements    | `Val`    | 1457.6 (1454.5 to 1466.7) | 1374.3 (1361.4 to 1404.2) |
| `list<u8>`, 65,536 elements    | direct   | 5.06 (5.04 to 5.17)       | 4.29 (4.28 to 4.31)       |
| `list<u32>`, 1,024 elements    | direct   | 3.88 (3.85 to 4.11)       | 2.55 (2.54 to 2.56)       |
| `list<u32>`, 4,096 elements    | `Val`    | 97.4 (97.1 to 97.7)       | 60.6 (60.0 to 61.3)       |
| `list<u32>`, 16,384 elements   | direct   | 18.4 (18.3 to 18.5)       | 10.8 (10.7 to 11.4)       |
| `list<point>`, 16 records      | `Val`    | 6.08 (6.07 to 6.23)       | 5.49 (5.40 to 5.59)       |
| `list<point>`, 256 records     | `Val`    | 49.9 (49.7 to 49.9)       | 76.5 (76.3 to 76.6)       |
| `list<point>`, 4,096 records   | `Val`    | 745.5 (743.9 to 748.5)    | 1223.9 (1222.7 to 1225.0) |
| `string`, 65,536 bytes         | direct   | 7.24 (7.21 to 7.39)       | 5.63 (5.29 to 6.76)       |
| one `u32` in and one `u32` out | direct   | 2.00 (1.97 to 2.17)       | 1.60 (1.49 to 1.74)       |

The suite sizes its direct `list<u32>` benchmarks to move the same bytes as its
`list<u8>` benchmarks. It has no direct `list<u32>` benchmark at 4,096 elements.
The direct crossing at 4,096 elements lies between the 1,024 and the 16,384
results. The direct crossing of `list<point>` needs the derive of this design,
so the table has only its `Val` results.

These numbers show three facts:

- On a list of bytes, the direct crossing is faster than the `Val` crossing. It
  is about 290 times faster on the web and about 320 times faster natively.
- On the `Val` crossing, a list of two-field records costs about 180 ns for each
  record on the web and about 300 ns natively. The direct crossing of a derived
  record removes this cost.
- On `list<point>` at 256 and 4,096 records, the native lane is slower than the
  web lane. The direct crossing removes the `Val` work on both lanes, so this
  fact does not change the design.

## The Type Check

### What It Compares

Each type that implements `ComponentType` compares itself with a type that a
component declares. The check walks the declared type and the Rust type
together. It compares in Wasmtime's order:

- A record: the count of fields, then each field in order. For each field, it
  compares the type, then the name.
- A variant: the count of cases, then each case in order. For each case, it
  compares the name, then whether the case has a payload, then the payload's
  type.
- An enum or a flags type: the count, then each name in order.
- A list, an option, a result, a tuple, a map, a fixed-length list, a stream, or
  a future: the kind, then each inner type.
- A resource: the identity, as the section on resources describes.

The check has no subtyping. A record with an extra field does not match. The
name of a field or a case is its WIT name. A host author gives a WIT name that
differs from the Rust name only with `#[component(name = "…")]`.

### When It Runs

The type check runs at two points:

- `Func::typed` compares the parameter tuple and the result tuple with the
  export.
- The linker compares each typed host function with the import that it
  satisfies, when it links a component.

The type check never runs on a call. A call trusts the check that ran when the
host acquired the function.

One comparison happens on a call, and it is not the type check. A `ResourceAny`
carries the identity of its type at run time. When the host lowers one, the
lowering compares that identity with the identity that the component declares.
Wasmtime does the same. A mismatch fails with the ABI error of [PDD009] for a
handle of the wrong resource type.

### What It Reports

A mismatch fails with `Error::TypeMismatch`. The error has a position, which
names where the mismatch occurred ([PDD008]). The position holds a path from the
function to the first part that does not match. The path is a list of steps.
Each step is one of these:

- A parameter or a result, by index.
- A field or a case, by the name that the component declares.
- A list element, a map key, or a map value.
- The payload of an option, the `ok` or `error` payload of a result, or the
  payload of a stream or a future.
- A tuple member, by index.

For example, the path to the second field of a record in the first parameter is
"parameter 0, then field `y`". At link time, the position also names the
interface and the item, as [PDD007] designs. Wasmtime gives this context as
text. The polyfill gives it as structured data.

### What It Costs

The type check costs time in proportion to the count of fields, cases, and inner
types that it walks. It allocates nothing when the types match. It builds the
path of a mismatch only when a mismatch occurs.

A typed function pays the type check once, when the host acquires it. A host
function pays it once for each link. A component with many typed host imports
pays it at each instantiation. A pre-instantiated component can pay it once, and
this design does not include one.

The benchmark suite measures the type check on its own, with two benchmarks:

- The acquisition of a typed function. It runs for a flat signature, for a
  record of 16 fields, and for a record inside a variant inside a list.
- The instantiation of a component whose imports are typed host functions over
  derived types, beside the same component with `func_new` host functions.

## Derived Types

### The Derive Macros

The polyfill has three derive macros with Wasmtime's names: `ComponentType`,
`Lift`, and `Lower`. A host author applies all three together. The style
attribute of a derived type is the one of `record`, `variant`, or `enum` that
tells the derive what WIT type to produce:

```rust
#[derive(ComponentType, Lift, Lower)]
#[component(record)]
struct Point {
    x: u32,
    y: u32,
}

#[derive(ComponentType, Lift, Lower)]
#[component(variant)]
enum Shape {
    #[component(name = "circle")]
    Circle(u32),
    #[component(name = "none")]
    Nothing,
}

#[derive(ComponentType, Lift, Lower, Debug)]
#[component(enum)]
#[repr(u8)]
enum Level {
    #[component(name = "info")]
    Info,
    #[component(name = "error")]
    Error,
}
```

The derive accepts the attributes and enforces the shape rules of Wasmtime's
derive:

- `#[component(record)]` applies to a struct with named fields.
- `#[component(variant)]` applies to an enum with at least one case. Each case
  has no payload or one unnamed payload.
- `#[component(enum)]` applies to an enum whose cases have no payload and no
  explicit discriminant. The enum carries `#[repr(u8)]`, `#[repr(u16)]`, or
  `#[repr(u32)]` to match its count of cases.
- `#[component(name = "…")]` on a field or a case gives its WIT name. Without
  it, the WIT name is the Rust name as written.
- A type with no style attribute, or with two, fails to compile.
- `#[component(flags)]` fails to compile with a message that names `flags!`.

Each refusal is a compile error with the meaning of Wasmtime's refusal.

### Flags

The `flags!` macro declares a flags type with Wasmtime's syntax:

```rust
flags! {
    Permissions {
        #[component(name = "read")]
        const READ;
        #[component(name = "write")]
        const WRITE;
    }
}
```

The macro emits Wasmtime's API for the type:

- One constant for each flag.
- `empty`, `all`, `contains`, `intersects`, and `as_array`.
- The bitwise operators `|`, `&`, `^`, and `!`, with their assigning forms.
- `Debug`, `Default`, `Clone`, `Copy`, `PartialEq`, and `Eq`.
- `ComponentType`, `Lift`, and `Lower`.

### The Crate Path

A crate that re-exports the polyfill tells the derive where the polyfill is with
`#[component(wcmp_crate = path)]`. The derive refuses Wasmtime's
`#[component(wasmtime_crate = path)]` with a message that names `wcmp_crate`.
Wasmtime's attribute names a path to Wasmtime. A pasted Wasmtime source that
uses it names the wrong crate. The refusal tells the author which attribute to
use. `flags!` takes no crate path, as in Wasmtime.

## Value Types

Each value type of the Component Model has the Rust forms below. Each form
implements `ComponentType`, `Lift`, and `Lower` unless the table says "lower
only".

| WIT type                            | Rust forms                                                         |
| ----------------------------------- | ------------------------------------------------------------------ |
| `bool`, `s8` to `u64`, `f32`, `f64` | `bool`, `i8` to `u64`, `f32`, `f64`                                |
| `char`                              | `char`                                                             |
| `string`                            | `String`, `Box<str>`, `Arc<str>`, and `str` (lower only)           |
| `list<T>`                           | `Vec<T>`, `Box<[T]>`, `Arc<[T]>`, and `[T]` (lower only)           |
| `list<T, N>`                        | `[T; N]`. The type check compares `N`.                             |
| `map<K, V>`                         | `HashMap<K, V>`                                                    |
| `option<T>`                         | `Option<T>`                                                        |
| `result<T, E>`                      | `Result<T, E>`. The unit type `()` matches a case with no payload. |
| `tuple<…>`                          | Rust tuples of 0 to 17 values                                      |
| `record`, `variant`, `enum`         | derived types                                                      |
| `flags`                             | types that `flags!` declares                                       |
| `own<R>`, `borrow<R>`               | `Resource<T>`, `ResourceDynamic`, and `ResourceAny`                |
| `stream<T>`, `future<T>`            | `StreamReader<T>`, `FutureReader<T>`, `StreamAny`, and `FutureAny` |
| `error-context`                     | `ErrorContext`                                                     |
| any other type                      | `&T`, `Box<T>`, and `Arc<T>` forward to `T` (lower only)           |

A tuple is a value as well as a parameter list. A tuple inside a record, a list,
or a result crosses as a WIT `tuple`.

## Resources

### Resource Type Identity

`ResourceType` is the identity of a resource type. It is `Copy` and `Eq`.
Equality compares identity and nothing else. It has three sources:

- `ResourceType::host::<T>()` gives the identity of the host resource that the
  Rust type `T` stands for. Each call with the same `T` gives the same identity.
- `ResourceType::host_dynamic(payload)` gives the identity of a host resource
  that the host keys on a `u32`. It serves a host that makes resource types at
  run time, for example from a list of names that it reads from WIT.
- Each instantiation gives a fresh identity to each guest resource, as [PDD012]
  designs.

The type reflection of the polyfill carries the identity. `ValueType::Own` and
`ValueType::Borrow` hold a `ResourceType`. Two resource types with the same WIT
name and different identities are different types everywhere in the polyfill.
`ResourceType` holds no name. An error message names a resource type by the name
that the component declares at the position of the error.

### Registration

A host registers a host resource on an instance of the linker with a name, a
`ResourceType`, and a destructor:

```rust
// `files` is the linker instance of the interface `example:files/types`.
files.resource("file", ResourceType::host::<File>(), |store, rep| {
    store.data_mut().table.delete(Resource::<File>::new_own(rep))?;
    Ok(())
})?;
```

The destructor receives a mutable context for the store and the rep. Through the
context, it reaches the host data. A host that uses one resource type in two
interfaces registers `ResourceType::host::<File>()` in each. The two
registrations have the same identity because the Rust type is the same.

### The Typed Handle

`Resource<T>` is a typed handle to a host resource whose identity is
`ResourceType::host::<T>()`. `T` is a marker. `Resource<T>` holds no `T`, so it
is `Send` and `Sync` for every `T`. It is not `Clone`.

A `Resource<T>` holds a rep and a state. It has Wasmtime's four states:

- A borrow. The host received it as a `borrow` parameter. It is valid until the
  host function returns.
- Owned and in no table. `Resource::new_own(rep)` makes one.
- Owned and moved into a guest. The host lowered it as an `own`.
- Owned and in the host table. The host lowered a `borrow` of it, and the
  polyfill tracks the borrow.

The host lowers an owned handle as an `own` once. A second lowering fails with
Wasmtime's message, "host resource already consumed". The host can lower an
owned handle as a `borrow` many times. The first borrow puts the handle in the
host table, so that the polyfill can track each borrow. Wasmtime does the same
([Wasmtime resource states]).

`Resource<T>` has Wasmtime's methods: `new_own`, `new_borrow`, `rep`, `owned`,
`try_from_resource_any`, and `try_into_resource_any`.

`ResourceDynamic` is the same handle for a type from `host_dynamic`. In place of
the marker `T`, it carries the `u32` of its type. It has Wasmtime's methods:
`new_own`, `new_borrow`, `rep`, `ty`, `owned`, `try_from_resource_any`, and
`try_into_resource_any`.

### The Dynamic Handle

`ResourceAny` is a handle to a resource of any type. It is the only form of a
handle to a guest resource. A host also uses it for a host resource whose type
it does not know at compile time. It holds an index into the host table and the
identity of its type. It records whether it is owned or borrowed.

`ResourceAny` has Wasmtime's methods: `ty`, `owned`, `resource_drop`,
`try_from_resource`, and `try_into_resource`. A host makes one from a typed
handle with `try_into_resource_any`. A host releases an owned `ResourceAny` with
`resource_drop`, which runs the destructor once, as [PDD017] designs. A host
that drops an owned `ResourceAny` without `resource_drop` leaks the resource
until the store drops, as in Wasmtime.

The host table refuses a stale handle. Each slot of the table has a generation,
and a handle records the generation of its slot. A handle whose slot was freed
and used again fails with Wasmtime's message, "host-owned resource was already
de-allocated". A handle whose rep differs from the rep of its slot fails with a
reason that names the mismatch.

`Val` holds a handle as `Val::Resource(ResourceAny)`, as in Wasmtime. The value
of `owned` tells an `own` from a `borrow`.

### ResourceTable

`ResourceTable` is a table of host values that the host keeps in its store data.
A host puts a value in it and receives a `Resource<T>` whose rep is the index of
the value. It has Wasmtime's API:

- `new`, `with_capacity`, `is_empty`, `max_capacity`, and `set_max_capacity`.
- `push`, `push_child`, `add_child`, and `remove_child`.
- `get`, `get_mut`, `get_any_mut`, and `delete`.
- `iter_entries`, `iter_children`, and `iter_mut`.

Its errors are Wasmtime's: `Full`, `NotPresent`, `WrongType`, and `HasChildren`.
They are cases of the polyfill's `ResourceTableError`. `Full` is also the error
of the store's limit on its own records ([PDD021]).

`ResourceTable` is a plain data structure. It is not the host table, and the two
do not share entries.

`push` and `push_child` require `T: Send` natively, as in Wasmtime. In the
browser, they do not require `Send`. This follows `Store::new` and the host
futures, which drop the same bound in the browser. A browser host often holds
JavaScript objects as resources, such as a DOM node or a `Response`. These
objects are not `Send`. If `ResourceTable` refused them, each browser host must
keep a second map. This is a difference from Wasmtime, and it applies to
`wasm32` only.

### Resources Inside Derived Types

A field of a derived type can be a `Resource<T>`, a `ResourceDynamic`, or a
`ResourceAny`. The derive treats it as any other field:

- A `Resource<T>` field matches an `own<R>` or a `borrow<R>` whose identity is
  `ResourceType::host::<T>()`. It never matches a guest resource.
- A `ResourceDynamic` field matches an `own<R>` or a `borrow<R>` whose identity
  is from `host_dynamic`. Its lowering compares the `u32` of the type.
- A `ResourceAny` field matches any `own` or `borrow`. Its lowering compares the
  resource type.
- A `borrow` that a host function receives inside a derived type is valid until
  the host function returns, as a `borrow` parameter is.
- A `borrow` never appears in a result. Component validation refuses it, so the
  derive needs no rule of its own.

A lift can fail after it moved an `own` field out of the guest's table. For
example, a later field can hold a string that is not valid UTF-8. The lift
traps, and the trap poisons the store, as [PDD023] designs. Nothing can reach
the moved handle after that, so nothing observable leaks. Wasmtime behaves the
same.

## Both Targets

The derive emits the same code for every target. The three traits, the value
types, the resource handles, and `ResourceTable` behave the same natively and on
`wasm32-unknown-unknown`. The only difference is the `Send` bound of
`ResourceTable::push` and `ResourceTable::push_child`.

The tests of this design run on both lanes. The web lane comes first in every
report and every benchmark, because the browser is the main target of the
polyfill.

## What Generated Code Relies On

Generated code can rely on each item below. Each has Wasmtime's name and shape,
under `wcmp` in place of `wasmtime::component`:

- The derive macros `ComponentType`, `Lift`, and `Lower`, and the attributes
  `record`, `variant`, `enum`, `name`, and `wcmp_crate`.
- The `flags!` macro.
- The constants `SIZE32` and `ALIGN32` on each type that implements
  `ComponentType`.
- `ComponentNamedList` for tuples of 0 to 17 values.
- `Resource<T>`, `ResourceAny`, `ResourceType::host`, and
  `LinkerInstance::resource` with a name, a type, and a destructor.
- `StreamReader<T>`, `FutureReader<T>`, `ErrorContext`, `HashMap<K, V>`, and
  `[T; N]` as the forms of their WIT types.
- `LinkerInstance::func_wrap` and `LinkerInstance::func_wrap_concurrent` with a
  parameter tuple and a result tuple.
- `TypedFunc<Params, Results>`, with `call` and `call_concurrent`.

Generated code does not name the hidden items of the three traits. It does not
name the lift context or the lower context. Generated code that calls a function
or receives a host call uses the call surfaces of [PDD010] and [PDD011]. There
it differs from Wasmtime's generated code.

## Replaced Names

Each concept of this design has one name. The polyfill has no other public name
for these concepts:

- The three traits and `ComponentNamedList` are the typed value traits. No
  `ComponentValue`, `ComponentParameters`, or `ComponentResult` exists.
- `ResourceAny` is the dynamic handle. No `ResourceHandle` exists.
- `ResourceType` is the identity. No `ResourceTypeId` exists, and no
  `ResourceType` holds a name or compares by name.
- A registration takes a `ResourceType` and a destructor. No `HostResource<T>`
  or `LinkerInstance::resource_with` exists, and no registration mints an
  identity.
- A host makes an owned handle with `Resource::new_own` or
  `ResourceDynamic::new_own`. No `Store`, `StoreContext`, or `HostCall` mints a
  handle.
- A host releases a handle with `ResourceAny::resource_drop`. No
  `Store::resource_drop` exists.
- `Val::Resource` holds a handle. No `Val::Own` or `Val::Borrow` exists.

A typed value has no conversion to or from a `Val`, and no method that gives its
`ValueType`, except `ErrorContext`, which has Wasmtime's `into_val` and
`from_val`. Wasmtime has no other such conversion. Every other typed handle
reaches a `Val` through its untyped form, as `Resource<T>` does through
`ResourceAny`. A host that needs both forms uses the typed path for typed values
and the dynamic path for `Val`.

## Revisions to Earlier Designs

[PDD003] is revised in place. It lists the derive macros, `flags!`,
`Resource<T>`, `ResourceAny`, and `ResourceTable` as "Implement".

[PDD008] and [PDD011] are revised in place. `Val` is the value of the dynamic
path, and a typed host function crosses its values directly.

[PDD009] is revised in place. The registration takes a `ResourceType`, and the
host meets handles as `Resource<T>` and `ResourceAny`.

[PDD010] is revised in place. The typed path uses the three traits, and a typed
function returns a tuple of results.

[PDD012] and [PDD013] are revised in place. The host holds a handle to a guest
resource as `ResourceAny`, and the identity of a resource type is
`ResourceType`.

[PDD017] and [PDD023] are revised in place. The host releases a resource with
`ResourceAny::resource_drop`.

[PDD018], [PDD020], and [PDD021] are revised in place. Their signatures name the
three traits and `ComponentNamedList`, and no host call mints a handle through
the store.

## User Stories

A developer moves a Wasmtime host to the browser.

> The developer's host defines a record, a variant, and a flags type with
> Wasmtime's derive macros and `flags!`. It keeps its files in a
> `ResourceTable`. In the code for these types and the table, the developer
> changes each `use wasmtime::component::…` line to `use wcmp::…`. The host
> compiles for `wasm32-unknown-unknown` and runs in Chrome with the same results
> as on Wasmtime.

A developer writes a typed host function over a record.

> The developer derives the traits on `Point` and registers a host function that
> takes `(Point,)` and returns `(u32,)`. A component that imports the function
> with a record of the same shape links. A component whose record names its
> second field `z` fails to link. The error's path is "parameter 0, then field
> `z`".

A developer passes a large list of records to a component in the browser.

> The developer calls a typed function with 4,096 derived records. The call
> builds no `Val`. The page measures the call. Its time is close to the time of
> a numeric list of the same size in bytes.

A developer keeps JavaScript objects as resources.

> The developer's browser host wraps each `fetch` `Response` in a host value and
> pushes it into a `ResourceTable`. The host passes the `Resource<Body>` to the
> guest as an `own`. When the guest drops it, the destructor deletes the entry.

A maintainer generates bindings from WIT.

> The generator emits derive macros, `flags!`, layout assertions, and
> `Resource<T>` fields with the names that Wasmtime's generated code uses. The
> output compiles against the polyfill with `wcmp` as the crate path.

## Test Cases

A record and a variant cross in both directions as Rust types on both targets. A
typed export takes a derived record and returns a derived variant. A typed host
function takes a derived variant and returns a derived record. The test states
each expected Rust value. Each value that crosses equals the expected value.

The typed path builds no `Val`. A test counts allocations around a typed call
with a list of derived records, on both targets. The count of allocations is the
same for 16 records and for 4,096 records, apart from the allocation of the
result list.

A type mismatch is refused when the host acquires a typed export. A request for
a typed function whose record has a different field name, a different field
type, or an extra field fails with `Error::TypeMismatch`. The path names the
first wrong field. No call reaches the guest.

A type mismatch is refused when the linker links a typed host function. A
component whose import declares a different record fails to link with
`Error::TypeMismatch`. The position names the interface and the item, and the
path names the wrong field.

A type check compares resources by identity. Two interfaces each declare a
resource named `error`. The host registers the first with
`ResourceType::host::<A>()` and the second with `ResourceType::host::<B>()`. A
typed host function that takes `Resource<A>` links to a function of the first
interface and fails to link to a function of the second.

The derive enforces Wasmtime's shape rules. A compile-fail test covers each
refusal:

- A record on a tuple struct.
- A variant with no cases, and a variant with a case of two payloads.
- An enum without `#[repr]`, an enum whose `#[repr]` width does not match its
  count of cases, and an enum with an explicit discriminant.
- A type with no style attribute, and a type with two.
- `#[component(flags)]` and `wasmtime_crate`.

Each error names the rule that the type breaks.

The `name` attribute renames a field and a case. A derived record and a derived
variant with renamed parts cross both ways, and the type check uses the WIT
names.

A flags type crosses through `flags!`. A flags type with 1, 8, 9, and 32 flags
crosses in both directions. A 33-flag type has Wasmtime's layout, checked
without a guest, because the Component Model caps a flags type at 32 flags. The
test exercises each constant, `empty`, `all`, `contains`, `intersects`, and each
operator.

Every value type of the table crosses on both targets. A test crosses each Rust
form in the value types table in each direction that the table allows. A
fixed-length list with the wrong length fails the type check. A `Result` with
`()` matches a case with no payload.

A resource crosses as `Resource<T>`. The host makes a handle with
`Resource::new_own` and passes it to a guest as an `own`. A host function then
receives the guest's `borrow` of it. The rep matches at each step. A second
lowering of the same owned handle as an `own` fails with "host resource already
consumed".

A resource crosses as `ResourceDynamic`. The host registers two types with
`host_dynamic(1)` and `host_dynamic(2)`, and passes a handle of each to a guest
and back. The rep and the `u32` of the type match at each step. A handle of type
1 fails to lower where the component declares type 2.

A resource crosses as `ResourceAny`. The host receives an owned guest resource
as `ResourceAny` and passes it back to the guest. The host receives a second
owned guest resource and releases it with `resource_drop`. The destructor of the
second resource runs once. A `ResourceAny` of the wrong type fails when the host
lowers it. `try_into_resource` succeeds for the right `T` and fails for another.

A derived type holds `own` and `borrow` fields. A record has a `Resource<T>`
field and a `ResourceAny` field. It crosses into a host function with `borrow`
fields and out of a host function with `own` fields. The host keeps the
`ResourceAny` borrow past the return of the host function and lowers it into a
later call. That lowering fails with the invalid-handle ABI cause of [PDD009].

The host table refuses a stale handle. A `ResourceAny` whose slot was freed and
used again fails with "host-owned resource was already de-allocated".

`ResourceTable` matches Wasmtime. Each error occurs where Wasmtime gives it:

- `NotPresent` from `get` after `delete` of the same entry.
- `WrongType` from `get` with a `Resource` of another `T` at the same index.
- `Full` from `push` past `set_max_capacity`.
- `HasChildren` from `delete` of a parent that has a child from `push_child`.

In the browser, `ResourceTable` holds a value that is not `Send`.

A resource type has one identity in every interface. A host registers
`ResourceType::host::<T>()` under two interfaces, A and B. A function of A
returns an `own` handle to the host. The host passes it to a function of B, and
the rep matches.

The direct crossing is measured. The benchmark suite adds a typed `list<point>`
round trip at 16, 256, and 4,096 records beside the `Val` benchmark of the same
list. Both lanes run it, web first. The report states each median with its
spread and with the conditions of the measurement section. The typed benchmark
is faster than the `Val` benchmark at every size on both lanes.

The type check is measured. The benchmark suite adds the acquisition of a typed
function for three signatures and the instantiation of a component with typed
host imports. The report states each cost with its conditions. A test counts the
runs of the type check around 1,000 calls of one typed function. The count does
not change.

## References

- [PDD003], the compatibility outlook, revised in place by this design.
- [PDD007], linking and instantiation.
- [PDD008], the Canonical ABI and host functions, revised in place by this
  design.
- [PDD009], resources, revised in place by this design.
- [PDD010], the public API before WASI 0.3, revised in place by this design.
- [PDD011], host registration and the store context, revised in place by this
  design.
- [PDD012], locally defined resources, revised in place by this design.
- [PDD013], shared resource type identity, revised in place by this design.
- [PDD017], resource disposal, revised in place by this design.
- [PDD018], the concurrency runtime model, revised in place by this design.
- [PDD020], subtasks and the async import, revised in place by this design.
- [PDD021], streams and futures, revised in place by this design.
- [PDD023], cancellation and the poisoned store, revised in place by this
  design.
- [PDD025], the runtime layer.
- [Wasmtime] at `v49.0.0-rc.1`: its [traits][Wasmtime traits], its [type
  check][Wasmtime type check], its [`Func::typed`][Wasmtime typed], its
  [derive][Wasmtime derive], its [values][Wasmtime values], its
  [types][Wasmtime types], its [linker][Wasmtime linker] and
  [matching][Wasmtime matching], its [resource type][Wasmtime resource type],
  [resource registration][Wasmtime resource registration], [resource
  states][Wasmtime resource states] and [consumed handles][Wasmtime consumed],
  [`ResourceDynamic`][Wasmtime resource dynamic],
  [`ResourceAny`][Wasmtime resource any], [stale handles][Wasmtime stale], and
  [`ResourceTable`][Wasmtime resource table], and the code that its
  [`bindgen!`][Wasmtime bindgen] emits.

[PDD003]: ./PDD003%20Compatibility%20Outlook.md
[PDD007]: ./PDD007%20Linking%20and%20Instantiation.md
[PDD008]: ./PDD008%20Canonical%20ABI%20and%20Host%20Functions.md
[PDD009]: ./PDD009%20Resources.md
[PDD010]: ./PDD010%20Pre-wasip3%20Public%20API.md
[PDD011]:
  ./PDD011%20Host%20Registration:%20Root%20Namespace%20and%20Store%20Context.md
[PDD012]: ./PDD012%20Locally-Defined%20Resources.md
[PDD013]: ./PDD013%20Shared%20Resource%20Type%20Identity.md
[PDD017]: ./PDD017%20Resource%20Disposal.md
[PDD018]: ./PDD018%20Concurrency%20Runtime%20Model.md
[PDD020]: ./PDD020%20Subtasks%20and%20the%20Async%20Import.md
[PDD021]: ./PDD021%20Streams%20and%20Futures.md
[PDD023]:
  ./PDD023%20Cancellation,%20Error%20Contexts,%20and%20the%20Poisoned%20Store.md
[PDD025]: ./PDD025%20Runtime%20Layer.md
[Wasmtime]: https://github.com/bytecodealliance/wasmtime
[Wasmtime traits]:
  https://github.com/bytecodealliance/wasmtime/blob/v49.0.0-rc.1/crates/wasmtime/src/runtime/component/func/typed.rs#L498-L736
[Wasmtime type check]:
  https://github.com/bytecodealliance/wasmtime/blob/v49.0.0-rc.1/crates/wasmtime/src/runtime/component/func/typed.rs#L2205-L2340
[Wasmtime typed]:
  https://github.com/bytecodealliance/wasmtime/blob/v49.0.0-rc.1/crates/wasmtime/src/runtime/component/func.rs#L170-L187
[Wasmtime derive]:
  https://github.com/bytecodealliance/wasmtime/blob/v49.0.0-rc.1/crates/component-macro/src/component.rs#L41-L345
[Wasmtime values]:
  https://github.com/bytecodealliance/wasmtime/blob/v49.0.0-rc.1/crates/wasmtime/src/runtime/component/values.rs#L92
[Wasmtime types]:
  https://github.com/bytecodealliance/wasmtime/blob/v49.0.0-rc.1/crates/wasmtime/src/runtime/component/types.rs#L781
[Wasmtime linker]:
  https://github.com/bytecodealliance/wasmtime/blob/v49.0.0-rc.1/crates/wasmtime/src/runtime/component/linker.rs#L236-L241
[Wasmtime matching]:
  https://github.com/bytecodealliance/wasmtime/blob/v49.0.0-rc.1/crates/wasmtime/src/runtime/component/matching.rs#L69-L80
[Wasmtime resource type]:
  https://github.com/bytecodealliance/wasmtime/blob/v49.0.0-rc.1/crates/wasmtime/src/runtime/component/resources/ty.rs#L44-L62
[Wasmtime resource registration]:
  https://github.com/bytecodealliance/wasmtime/blob/v49.0.0-rc.1/crates/wasmtime/src/runtime/component/linker.rs#L836-L841
[Wasmtime resource states]:
  https://github.com/bytecodealliance/wasmtime/blob/v49.0.0-rc.1/crates/wasmtime/src/runtime/component/resources/host.rs#L37-L60
[Wasmtime consumed]:
  https://github.com/bytecodealliance/wasmtime/blob/v49.0.0-rc.1/crates/wasmtime/src/runtime/component/resources/host.rs#L200
[Wasmtime resource dynamic]:
  https://github.com/bytecodealliance/wasmtime/blob/v49.0.0-rc.1/crates/wasmtime/src/runtime/component/resources/host_dynamic.rs#L66-L141
[Wasmtime resource any]:
  https://github.com/bytecodealliance/wasmtime/blob/v49.0.0-rc.1/crates/wasmtime/src/runtime/component/resources/any.rs#L240-L298
[Wasmtime stale]:
  https://github.com/bytecodealliance/wasmtime/blob/v49.0.0-rc.1/crates/wasmtime/src/runtime/component/resources/host_tables.rs#L180
[Wasmtime resource table]:
  https://github.com/bytecodealliance/wasmtime/blob/v49.0.0-rc.1/crates/wasmtime/src/runtime/component/resource_table.rs#L155-L268
[Wasmtime bindgen]:
  https://github.com/bytecodealliance/wasmtime/blob/v49.0.0-rc.1/crates/wit-bindgen/src/lib.rs#L2096-L2101
