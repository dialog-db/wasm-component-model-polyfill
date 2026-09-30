//! The seam between the polyfill and the runtime layer it runs core
//! WebAssembly through.
//!
//! This module is the one place in the polyfill that names the
//! runtime layer's crates. Every other module reaches a runtime-layer
//! type through the names below, so pointing the polyfill at another
//! runtime layer changes this file alone. A flake check fails when a
//! source file of the crate other than this one names a runtime-layer
//! crate. The module is workspace-private: nothing inside it is
//! re-exported by `lib.rs`, and the one public signature that names
//! a runtime-layer item is the engine's constructor, whose parameter
//! is the backend the host chose.
//!
//! The polyfill has no backend of its own. The host hands one to
//! [`Engine::with_backend`](crate::Engine::with_backend), and the
//! runtime layer holds it behind dynamic dispatch, so no type here
//! carries a backend type parameter. The polyfill's component-level
//! work is implemented above the runtime layer, and never delegated
//! to a backend's own component runtime.
//!
//! Every backend calls a host function at any depth: a host function
//! already on the stack is called again with no more ceremony than
//! any other, and the arguments and results of each call belong to
//! that call alone.

use core::future::Future;
use core::pin::pin;
use core::task::{Context, Poll, Waker};
use std::collections::HashMap;

use crate::error::{Error, InstantiationError};

/// The type of a memory, which only the tests make one from.
#[cfg(test)]
pub use wcmp_wasm_core::MemoryType;
pub use wcmp_wasm_core::backend::Backend;
pub use wcmp_wasm_core::{
    AsContextMut, Caller, Capabilities, Capability, Engine, Error as RuntimeError, Extern,
    ExternType, Func, FuncType, Global, GlobalType, HeapType, Instance, MaybeSend, Memory, Module,
    Mutability, ResumableCall, Store, StoreContextMut, SuspendedCall, Table, TrapKind, Val,
    ValType,
};

/// The backend the crate's own tests hand their engines: Wasmtime
/// natively, and the browser's engine in the browser.
#[cfg(all(test, not(target_arch = "wasm32")))]
pub fn test_backend() -> impl Backend {
    wcmp_wasm_core_wasmtime::Wasmtime::new().expect("Wasmtime makes an engine")
}

/// The backend the crate's own tests hand their engines: Wasmtime
/// natively, and the browser's engine in the browser.
#[cfg(all(test, target_arch = "wasm32"))]
pub fn test_backend() -> impl Backend {
    wcmp_wasm_core_web::Web::new()
}

/// A backend that declares host suspension, for the crate's tests of
/// the host-suspension provider: Wasmi natively, and the browser's
/// engine in the browser.
#[cfg(all(test, not(target_arch = "wasm32")))]
pub fn test_suspending_backend() -> impl Backend {
    wcmp_wasm_core_wasmi::Wasmi::new()
}

/// A backend that declares host suspension, for the crate's tests of
/// the host-suspension provider: Wasmi natively, and the browser's
/// engine in the browser.
#[cfg(all(test, target_arch = "wasm32"))]
pub fn test_suspending_backend() -> impl Backend {
    wcmp_wasm_core_web::Web::new()
}

/// The imports of a core instantiation, by their two names.
///
/// The runtime layer has no linker: an instantiation takes one
/// extern for each import of the module, in the module's order.
/// The polyfill collects what it links by name, and
/// [`resolve`](Self::resolve) lays the names out in that order.
#[derive(Default)]
pub struct Imports {
    map: HashMap<(String, String), Extern>,
}

impl Imports {
    /// Link `value` as the import `module` `name`, in place of any
    /// extern linked under the two names before.
    pub fn define(&mut self, module: &str, name: &str, value: impl Into<Extern>) {
        self.map
            .insert((module.to_owned(), name.to_owned()), value.into());
    }

    /// The extern linked as the import `module` `name`, if any.
    pub fn get(&self, module: &str, name: &str) -> Option<Extern> {
        self.map.get(&(module.to_owned(), name.to_owned())).copied()
    }

    /// One extern for each import of `module`, in its order. An import
    /// nothing was linked for is a link error that names it.
    pub fn resolve(&self, module: &Module) -> Result<Vec<Extern>, RuntimeError> {
        module
            .imports()
            .map(|import| {
                self.get(import.module(), import.name())
                    .ok_or_else(|| RuntimeError::Link {
                        module: import.module().to_owned(),
                        name: import.name().to_owned(),
                        message: "unknown import".to_owned(),
                    })
            })
            .collect()
    }
}

/// Instantiate `module` in `store` with `imports`, laid out in the
/// module's order.
pub async fn instantiate(
    store: impl AsContextMut,
    module: &Module,
    imports: &Imports,
) -> Result<Instance, RuntimeError> {
    let imports = imports.resolve(module)?;
    Instance::instantiate(store, module, &imports).await
}

/// The output of `future` where it completes on its first poll.
///
/// A backend whose engine works synchronously, such as Wasmtime,
/// finishes a compile and an instantiation on the first poll of its
/// future. The polyfill reaches for this where a step that must stay
/// synchronous, such as making a store, needs the runtime layer's
/// asynchronous entry, and only on a backend that finishes at once.
pub fn at_once<F: Future>(future: F) -> Option<F::Output> {
    match pin!(future).poll(&mut Context::from_waker(Waker::noop())) {
        Poll::Ready(output) => Some(output),
        Poll::Pending => None,
    }
}

/// The error a runtime-layer failure is carried as.
///
/// A trap that a host function of the polyfill raised carries the
/// host function's own error, which comes back unwrapped, so a
/// structured error a trampoline failed with stays reachable by
/// downcast. Every other failure, a guest's trap among them, keeps
/// the runtime layer's error, whose trap kind has one message on
/// every backend.
pub fn into_anyhow(error: RuntimeError) -> anyhow::Error {
    match error {
        RuntimeError::Trap(TrapKind::Host(error)) => error,
        error => anyhow::Error::new(error),
    }
}

/// A failure a guest call can end in: the runtime layer's own, or
/// one the polyfill carries already.
pub trait SubstrateCause {
    /// The failure, as the polyfill carries it.
    fn into_cause(self) -> anyhow::Error;
}

impl SubstrateCause for RuntimeError {
    fn into_cause(self) -> anyhow::Error {
        into_anyhow(self)
    }
}

impl SubstrateCause for anyhow::Error {
    fn into_cause(self) -> anyhow::Error {
        self
    }
}

/// The polyfill error a runtime-layer failure of a guest call
/// becomes. Workspace-internal.
pub fn substrate_failure(error: impl SubstrateCause) -> Error {
    Error::from(InstantiationError::SubstrateFailure(error.into_cause()))
}

/// The data of a store that counts the host functions running in it.
pub trait HostFrames {
    /// How many host functions run in the store now: the frames of
    /// host code that lie between the guest code on the stack.
    fn host_frames(&mut self) -> &mut usize;
}

/// A host function of type `ty` in `store`, whose body is `func`.
///
/// The body reaches the store the calling guest runs in through the
/// context it is handed, as the runtime layer's caller lends it. An
/// error from the body traps the guest, and no guest can catch the
/// trap. The store counts the body as a host frame for as long as it
/// runs, which is how the polyfill knows whether only WebAssembly
/// lies between the start of a thread's stack and a suspension.
pub fn host_func<T: HostFrames + 'static>(
    mut store: impl AsContextMut<Data = T>,
    ty: FuncType,
    func: impl Fn(StoreContextMut<'_, T>, &[Val], &mut [Val]) -> anyhow::Result<()>
    + Send
    + Sync
    + 'static,
) -> Result<Func, Error> {
    Func::new(
        store.as_context_mut(),
        ty,
        move |mut caller: Caller<'_, T>, params, results| {
            *caller.data_mut().host_frames() += 1;
            let result = func(caller.as_context_mut(), params, results);
            *caller.data_mut().host_frames() -= 1;
            result
        },
    )
    .map_err(substrate_failure)
}

/// A runtime-layer value that a public type of the polyfill holds: the
/// engine, or a compiled module.
///
/// The runtime layer makes neither `Send` nor `Sync` in the browser,
/// because the browser's backend holds JavaScript values there. The
/// public types of the polyfill are both on every target, and a host
/// function, which must be `Send` and `Sync` on every target, captures
/// them. On `wasm32` without the atomics feature there is exactly one
/// thread, so no value can reach another thread, and the wrapper
/// declares both there. On every other target it is `Send` and `Sync`
/// exactly where the value is.
#[derive(Clone, Debug)]
pub struct Shared<V>(V);

impl<V> Shared<V> {
    /// Hold `value`.
    pub fn new(value: V) -> Self {
        Self(value)
    }
}

impl<V> core::ops::Deref for Shared<V> {
    type Target = V;

    fn deref(&self) -> &V {
        &self.0
    }
}

// SAFETY: `wasm32` without the atomics feature has one thread, and no
// way to make another, so no value can be sent to or shared with one.
#[cfg(all(target_arch = "wasm32", not(target_feature = "atomics")))]
unsafe impl<V> Send for Shared<V> {}

// SAFETY: as for `Send` above.
#[cfg(all(target_arch = "wasm32", not(target_feature = "atomics")))]
unsafe impl<V> Sync for Shared<V> {}
