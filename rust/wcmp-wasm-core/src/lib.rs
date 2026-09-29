#![warn(missing_docs)]

//! The runtime layer of the Wasm Component Model Polyfill: one trait over
//! core WebAssembly, and the types every backend of that trait shares.
//!
//! A backend runs core WebAssembly on one engine, such as the browser's
//! engine, Wasmi, or Wasmtime. Each backend is its own crate. It implements
//! [`backend::Backend`], and a host hands a value of it to
//! [`Engine::with_backend`]. From then on the host sees only the types of
//! this crate. No public type here names a backend or carries a type
//! parameter for one, so one binary can hold engines over two backends at
//! once.
//!
//! The types follow Wasmtime's core API in name and in ownership:
//!
//! - [`Engine`] holds the backend and its [`Capabilities`], which are fixed
//!   for the life of the engine.
//! - [`Module`] is a compiled module. [`Module::compile`] is asynchronous on
//!   every backend. [`Module::new`] compiles synchronously, for small
//!   modules a backend or a host generates.
//! - [`Store`] owns every instance and every object an instance makes, and
//!   the host's own data. [`Caller`], [`StoreContext`], and
//!   [`StoreContextMut`] reach a store from inside a host function and from
//!   generic code, through [`AsContext`] and [`AsContextMut`].
//! - [`Instance`], [`Func`], [`Memory`], [`Global`], [`Table`], and [`Tag`]
//!   are handles to objects a store owns. An [`Extern`] is one of them. There
//!   is no linker: [`Instance::instantiate`] takes the imports of a module as
//!   an ordered list of externs.
//! - [`Val`] is a value that crosses the boundary of a module. The
//!   references among its cases are [`Func`], [`ExternRef`], [`AnyRef`]
//!   (with [`I31`]), [`ExnRef`], and [`ContRef`].
//!
//! # The boundary type model
//!
//! [`ValType`], [`RefType`], and [`HeapType`] describe every value type of
//! Wasm 3.0. A concrete heap type is a [`TypeHandle`], which the host can
//! print and compare but not look inside. The runtime layer describes only
//! the boundary of a module, its imports and exports, through
//! [`ImportType`], [`ExportType`], and [`ExternType`]. It checks no
//! subtypes: the engine does that when it links a module.
//!
//! # Capabilities
//!
//! The floor is Wasm 2.0: every backend implements it. Every Wasm feature
//! above the floor is a [`Capability`], a name from a fixed lexicon, and a
//! backend declares only the capabilities it implements faithfully. A method
//! that needs a capability exists on every backend. Where the backend lacks
//! the capability, the method returns [`Error::Unsupported`] with its name.
//!
//! # Host suspension
//!
//! A suspending host function ([`Func::new_suspending`]) can answer "not
//! yet" in place of its results. A call made with [`Func::call_resumable`]
//! then ends as [`ResumableCall::Suspended`], with a [`SuspendedCall`] that
//! the host resumes later with the results of the host function. Both steps
//! are asynchronous, because the browser delivers the end of a resumed call
//! through a promise.
//!
//! # Traps and errors
//!
//! Every fallible method returns an [`Error`]. A trap is [`Error::Trap`],
//! with a [`TrapKind`] whose names and messages are Wasmtime's on every
//! backend.

#[macro_use]
mod macros;

mod call;
mod capability;
mod checks;
mod contract;
mod engine;
mod error;
mod externs;
mod internal;
mod module;
mod store;
mod types;
mod values;

pub use crate::call::{ResumableCall, SuspendedCall};
pub use crate::capability::{Capabilities, Capability};
pub use crate::contract::{MaybeSend, MaybeSync};
pub use crate::engine::Engine;
pub use crate::error::{Error, Result, TrapKind};
pub use crate::externs::{Extern, Func, Global, Instance, Memory, Table, Tag};
pub use crate::module::{ExportType, ImportType, Module};
pub use crate::store::{AsContext, AsContextMut, Caller, Store, StoreContext, StoreContextMut};
pub use crate::types::{
    ExternType, FuncType, GlobalType, HeapType, MemoryType, Mutability, RefType, TableType,
    TagType, TypeHandle, ValType,
};
pub use crate::values::{AnyRef, ContRef, ExnRef, ExternRef, I31, Val};

pub mod backend {
    //! The contract a backend implements.
    //!
    //! A backend is a crate that runs core WebAssembly on one engine. It
    //! implements four traits:
    //!
    //! - [`Backend`] for the engine: its capabilities, its compiles, and its
    //!   stores.
    //! - [`BackendModule`] for a compiled module: the boundary of the module.
    //! - [`BackendStore`] for a store: every operation on an instance or an
    //!   object the store owns. The host functions of the store receive the
    //!   store as `&mut dyn BackendStore` while a guest calls them.
    //! - [`BackendSuspendedCall`] for a call that a suspending host function
    //!   set aside, where the backend declares
    //!   [`host_suspension`](crate::Capability::HostSuspension). The store
    //!   resumes the call, through [`BackendStore::resume_call`], so the
    //!   backend reaches its own concrete store when it resumes.
    //!
    //! A backend names the objects of a store with handles. It makes each
    //! handle with [`RawHandle::from_raw`], from the [`StoreId`] its
    //! [`StoreData`] carries and an index of its own choosing, and it reads
    //! the index back with [`RawHandle::index`]. A concrete heap type is a
    //! [`TypeHandle`](crate::TypeHandle) that the backend makes with
    //! [`RawTypeHandle::from_raw`].
    //!
    //! The engine checks what it can before it reaches a backend: that a
    //! handle belongs to the store it is used with, that a module belongs to
    //! the engine of the store, and that the backend declares a capability a
    //! method needs. A backend still validates every index it receives,
    //! because a handle is plain data, and returns a structured error for one
    //! it does not know. A backend never panics on a path a host can reach.

    pub use crate::contract::{
        Backend, BackendModule, BackendStore, BackendSuspendedCall, BoxFuture, HostFunc, RawHandle,
        RawTypeHandle,
    };
    pub use crate::store::{StoreData, StoreId};
}

// The crate's own unit tests run in a browser in the web lane, as every
// other test binary of the workspace does.
#[cfg(all(test, target_arch = "wasm32"))]
wasm_bindgen_test::wasm_bindgen_test_configure!(run_in_browser);
