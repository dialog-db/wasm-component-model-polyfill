#![warn(missing_docs)]
// An item the crate reaches only from its own tests is not dead
// code, and the plain library build cannot tell: its reachability
// walk starts at the public API and never enters a `cfg(test)`
// module. The test build compiles every line the library build does
// and the tests besides, so it is the build that answers the
// question, and `--all-targets` runs the lint there.
#![cfg_attr(not(test), allow(dead_code))]

//! A polyfill that brings the WebAssembly Component Model (wasip3) to
//! web browsers where only Wasm Core is presently supported.
//!
//! The crate exposes:
//!
//! - [`Engine`], [`EngineConfig`], and [`Store`] — the compilation
//!   context with the feature gates it validates against, and the
//!   owner of guest state, the foundations every higher-level type
//!   is built against.
//! - [`SuspendProviderKind`] — which provider, if any, lets an
//!   engine set a blocked guest thread aside and resume it later.
//! - [`Component`] — a parsed component value with accessors over
//!   its declared imports and exports.
//! - [`PackageName`] and [`InterfaceIdentifier`] — the addressing
//!   surface for component imports and exports.
//! - [`ValueType`] — the structural identity of every value type a
//!   parsed component can declare.
//! - [`Linker`], [`LinkerInstance`], [`Instance`], and [`Func`] —
//!   the build-up and runtime surface for linking a component
//!   against a host environment, instantiating it into a [`Store`],
//!   and calling its exports.
//! - [`InstanceExports`], [`ExportInstance`], and [`ExportLookup`] —
//!   the export navigator that walks a component's export tree and
//!   reaches function exports nested inside an instance-typed
//!   export, whether the instance is published under a WIT interface
//!   identifier or a plain name.
//! - [`Module`], [`CoreInstance`], and [`CoreExtern`] — the handle
//!   for a core module a component exports or the host loads from
//!   bytes, described by [`ModuleImport`], [`ModuleExport`],
//!   [`CoreExternType`], and [`CoreValueType`], instantiated by the
//!   host with imports it supplies, or registered through
//!   [`LinkerInstance::module`] for a component that imports it.
//! - [`TypedFunc`] — the typed counterpart to [`Func`] obtained
//!   through [`Func::typed`], whose Rust parameter tuple and return
//!   type are checked against the export's component-level
//!   signature at acquisition.
//! - [`Val`], [`ValField`], and [`ResourceHandle`] — the polyfill's
//!   component-level value enum and its compound-variant payloads.
//! - [`Error`] and [`Result`] — the polyfill's single error enum and
//!   the `Result` alias every public function returns.
//!
//! # Example
//!
//! Consider a component built from the following WIT:
//!
//! ```wit
//! package test:guest;
//!
//! interface foo {
//!     // Selects the item in position n within list x
//!     select-nth: func(x: list<string>, n: u32) -> string;
//! }
//!
//! world guest {
//!     export foo;
//! }
//! ```
//!
//! The component can be loaded into the polyfill and invoked as
//! follows:
//!
//! ```rust
//! # use wcmp_macros::component;
//! # // The component's WAT is assembled into bytes inline at compile
//! # // time by the polyfill's `component!` macro. The full listing is
//! # // hidden for readability — it is the wit-bindgen-style canonical-
//! # // ABI body a real guest would emit for `select-nth`, ending in a
//! # // single instance export named after the WIT interface above.
//! # const WASM: &[u8] = component!(r#"
//! #     (component
//! #       (core module $m
//! #         (memory (export "memory") 1)
//! #         (global $bump (mut i32) (i32.const 16))
//! #         (func $cabi-realloc (export "cabi_realloc")
//! #               (param i32 i32 i32 i32) (result i32)
//! #           (local $ptr i32)
//! #           ;; Round the bump pointer up to the alignment asked
//! #           ;; for, as a real allocator does: the canonical ABI
//! #           ;; reads back what it stores at that alignment, and
//! #           ;; refuses a pointer that does not have it.
//! #           (global.set $bump
//! #             (i32.and
//! #               (i32.add (global.get $bump)
//! #                        (i32.sub (local.get 2) (i32.const 1)))
//! #               (i32.sub (i32.const 0) (local.get 2))))
//! #           global.get $bump
//! #           local.set $ptr
//! #           global.get $bump
//! #           local.get 3
//! #           i32.add
//! #           global.set $bump
//! #           local.get $ptr)
//! #         (func (export "select-nth")
//! #               (param $list-ptr i32) (param $list-len i32) (param $n i32)
//! #               (result i32)
//! #           (local $ret i32)
//! #           (local $elem i32)
//! #           i32.const 0
//! #           i32.const 0
//! #           i32.const 4
//! #           i32.const 8
//! #           call $cabi-realloc
//! #           local.set $ret
//! #           local.get $list-ptr
//! #           local.get $n
//! #           i32.const 3
//! #           i32.shl
//! #           i32.add
//! #           local.set $elem
//! #           local.get $ret
//! #           local.get $elem
//! #           i32.load
//! #           i32.store
//! #           local.get $ret
//! #           local.get $elem
//! #           i32.load offset=4
//! #           i32.store offset=4
//! #           local.get $ret))
//! #       (core instance $i (instantiate $m))
//! #       (func $select-nth
//! #             (param "x" (list string)) (param "n" u32) (result string)
//! #         (canon lift (core func $i "select-nth")
//! #                    (memory (core memory $i "memory"))
//! #                    (realloc (core func $i "cabi_realloc"))))
//! #       (instance $foo (export "select-nth" (func $select-nth)))
//! #       (export "test:guest/foo" (instance $foo)))
//! # "#);
//! use wasm_component_model_polyfill::*;
//!
//! // Compiling, instantiating, and calling are awaited: the browser
//! // compiles large modules only asynchronously, and the same
//! // signatures serve native, where the futures complete at once.
//! #[tokio::main(flavor = "current_thread")]
//! async fn main() {
//!     // Create a new engine for instantiating a component. The
//!     // polyfill owns its substrate selection, so there is no
//!     // per-target engine type to thread in.
//!     let engine = Engine::new().unwrap();
//!
//!     // Create a store for managing component data and any custom
//!     // user-defined state.
//!     let mut store = Store::new(&engine, ()).unwrap();
//!
//!     // Parse the component bytes (assembled above) and load its
//!     // imports and exports.
//!     let component = Component::new(&engine, WASM).await.unwrap();
//!     // Create a linker that will be used to resolve the component's
//!     // imports, if any.
//!     let linker: Linker<()> = Linker::new(&engine);
//!     // Create an instance of the component using the linker.
//!     let instance = linker.instantiate(&mut store, &component).await.unwrap();
//!
//!     // Get the interface that the component exports.
//!     let interface = instance
//!         .exports()
//!         .instance("test:guest/foo")
//!         .unwrap();
//!     // Get the function for selecting a list element.
//!     let select_nth = interface
//!         .func("select-nth")
//!         .unwrap()
//!         .typed::<(Vec<String>, u32), String>()
//!         .unwrap();
//!
//!     // Create an example list to test upon.
//!     let example = ["a", "b", "c"]
//!         .iter()
//!         .map(ToString::to_string)
//!         .collect::<Vec<_>>();
//!
//!     assert_eq!(
//!         select_nth.call(&mut store, (example.clone(), 1)).await.unwrap(),
//!         "b",
//!     );
//! }
//! ```
//!
//! # What the crate keeps to itself
//!
//! Re-exporting a type from here carries its whole `pub` surface into
//! the public API, whether or not the values that surface hands back
//! can be named outside the crate: a method is reachable by method
//! syntax and a field by field syntax, however unnameable its type
//! is. So everything a re-exported type offers the crate and not its
//! callers is moved off that type — onto an extension trait, a
//! wrapper over a borrow, or a `Parts` struct the type is built from
//! — and the module those live in is private. What a caller outside
//! has is the re-exported name:
//!
//! ```rust
//! use wasm_component_model_polyfill::Func;
//! let _: Option<Func> = None;
//! ```
//!
//! What it does not have is the module behind them, which is
//! private, so no path through it resolves:
//!
//! ```compile_fail
//! use wasm_component_model_polyfill::internal::FuncParts;
//! fn parts() -> FuncParts {
//!     unimplemented!()
//! }
//! ```
//!
//! That block covers the module, not the individual seams and parts
//! inside it. It holds because `internal` is private, and it would
//! go on holding if one of them were separately re-exported by name
//! from this file. Keeping the internal surface out of the public
//! API is a rule about what the list of re-exports below admits, and
//! the list is the whole of the public API.
//!
//! Each type that carries such a seam says so in its own
//! documentation, and states it the same way: a block that compiles
//! against the public name beside a block that does not compile
//! against the internal one.

#[cfg(test)]
mod baseline;

mod abi;
mod backend;
mod component;
mod concurrency;
mod engine;
mod engine_config;
mod error;
mod executor;
mod identifier;
mod instance;
mod internal;
mod linker;
mod module;
mod resource;
mod store;
mod suspend_provider_kind;
mod types;
mod value;

pub use crate::component::{
    Component, ComponentExport, ComponentImport, ExternType, ExternalName, FunctionParameter,
    FunctionType, InstanceItem, InstanceType, ModuleType,
};
pub use crate::concurrency::{
    Accessor, Destination, EndKind, FutureAny, FutureConsumer, FutureProducer, FutureReader,
    GuardedFutureReader, GuardedStreamReader, HostFuture, Source, StreamAny, StreamConsumer,
    StreamProducer, StreamReader, StreamResult,
};
pub use crate::engine::Engine;
pub use crate::engine_config::EngineConfig;
pub use crate::error::{
    AbiCause, AbiError, AbiPosition, CopyCause, Error, ErrorContextCause, InstantiationError,
    LinkError, Result, ReturnMismatchKind, SchedulerCause, TaskCause, ThreadCause, TypeMismatch,
    TypeMismatchPosition, TypeRendering, WaitableCause,
};
pub use crate::identifier::{IdentifierParseError, InterfaceIdentifier, PackageName};
pub use crate::instance::{
    ExportInstance, ExportLookup, Func, Instance, InstanceExports, TypedFunc,
};
pub use crate::linker::{
    ComponentParameters, ComponentResult, ComponentValue, HostCall, HostResource, Linker,
    LinkerInstance,
};
pub use crate::module::{
    CoreExtern, CoreExternType, CoreInstance, CoreValueType, Module, ModuleExport, ModuleImport,
};
pub use crate::resource::{ResourceHandle, ResourceTypeId};
pub use crate::store::{Store, StoreContext};
pub use crate::suspend_provider_kind::SuspendProviderKind;
pub use crate::types::{
    EnumType, FixedLengthListType, FlagsType, FutureType, ListType, MapType, OptionType,
    PrimitiveType, RecordField, RecordType, ResourceType, ResultType, StreamType, TupleType,
    ValueType, VariantCase, VariantType,
};
pub use crate::value::{Val, ValField};

// The crate's own unit tests reach a browser in the web lane, where
// the scheduler's per-target wake after a yield is what they measure.
#[cfg(all(test, target_arch = "wasm32"))]
wasm_bindgen_test::wasm_bindgen_test_configure!(run_in_browser);
