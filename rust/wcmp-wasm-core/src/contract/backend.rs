//! The trait over core WebAssembly that every backend implements.

use crate::capability::Capabilities;
use crate::contract::{BackendModule, BackendStore, BoxFuture, MaybeSend, MaybeSync};
use crate::error::Result;
use crate::store::StoreData;

/// The engine side of a backend: its capabilities, its compiles, and its
/// stores.
///
/// A host hands a value of this trait to
/// [`Engine::with_backend`](crate::Engine::with_backend), and the engine
/// holds it behind dynamic dispatch from then on.
///
/// A backend implements the floor, Wasm 2.0, fully. It declares a
/// [`Capability`](crate::Capability) above the floor only where its engine
/// implements the feature faithfully.
pub trait Backend: MaybeSend + MaybeSync + 'static {
    /// The capabilities of the backend.
    ///
    /// The engine reads them once, when it is made, and keeps them for its
    /// life. A backend that probes its engine, as the browser backend does,
    /// probes before it is handed to the engine.
    fn capabilities(&self) -> Capabilities;

    /// Compiles `bytes` into a module, asynchronously.
    ///
    /// Each compile makes a module of its own. A backend keeps no cache of
    /// modules by their bytes. A module the engine refuses because it needs
    /// a capability the backend does not declare is
    /// [`Error::Unsupported`](crate::Error::Unsupported), with that
    /// capability. A module the engine refuses for any other reason is
    /// [`Error::Compile`](crate::Error::Compile), with the message of the
    /// engine.
    fn compile<'a>(&'a self, bytes: &'a [u8]) -> BoxFuture<'a, Result<Box<dyn BackendModule>>>;

    /// Compiles `bytes` into a module, synchronously.
    ///
    /// This compile is for small modules that a backend or a host
    /// generates. Where the engine refuses a synchronous compile of a large
    /// module, as the browser does, the backend returns a structured error.
    fn compile_sync(&self, bytes: &[u8]) -> Result<Box<dyn BackendModule>>;

    /// Makes a store that owns `data`.
    ///
    /// The store gives `data` back through
    /// [`BackendStore::data`] and [`BackendStore::data_mut`] for its whole
    /// life, and names its objects with the [`StoreId`](crate::backend::StoreId)
    /// that `data` carries.
    fn new_store(&self, data: StoreData) -> Result<Box<dyn BackendStore>>;
}
