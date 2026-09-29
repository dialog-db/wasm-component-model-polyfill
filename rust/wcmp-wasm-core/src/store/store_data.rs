//! What a backend keeps for the engine in each store.

use core::any::Any;
use core::fmt;

use crate::contract::MaybeSend;
use crate::engine::Engine;
use crate::internal::{StoreDataInternal, StoreIdInternal};
use crate::store::StoreId;

/// The host's data, erased: `Send` where the store must be.
#[cfg(not(target_arch = "wasm32"))]
type Erased = Box<dyn Any + Send>;

/// The host's data, erased: `Send` where the store must be.
#[cfg(target_arch = "wasm32")]
type Erased = Box<dyn Any>;

/// What a backend keeps for the engine in each store: the identity of the
/// store, its engine, and the host's data.
///
/// The engine makes one for each store and hands it to
/// [`Backend::new_store`](crate::backend::Backend::new_store). The backend
/// keeps it for the life of the store and gives it back through
/// [`BackendStore::data`](crate::backend::BackendStore::data), including
/// while a guest runs, which is how a host function reaches the host's
/// data. A backend cannot make one.
pub struct StoreData {
    id: StoreId,
    engine: Engine,
    value: Erased,
}

impl StoreData {
    /// The identity of the store. The backend makes the handles of its
    /// objects with it.
    pub fn id(&self) -> StoreId {
        self.id
    }

    /// The engine of the store.
    pub fn engine(&self) -> &Engine {
        &self.engine
    }
}

impl StoreDataInternal for StoreData {
    fn new<T: MaybeSend + 'static>(engine: Engine, value: T) -> Self {
        Self {
            id: StoreId::allocate(),
            engine,
            value: Box::new(value),
        }
    }

    fn user<T: 'static>(&self) -> &T {
        match self.value.downcast_ref::<T>() {
            Some(value) => value,
            // The engine makes the data of a store of `T` with a `T`, and
            // only the engine can make a `StoreData`. So the data a backend
            // gives back is a `T` unless the backend swapped the data of
            // two of its stores, which breaks the backend contract.
            None => unreachable!("a backend gave back the data of another store"),
        }
    }

    fn user_mut<T: 'static>(&mut self) -> &mut T {
        match self.value.downcast_mut::<T>() {
            Some(value) => value,
            // As in `user`: only a backend that swapped the data of two of
            // its stores reaches this arm.
            None => unreachable!("a backend gave back the data of another store"),
        }
    }
}

impl fmt::Debug for StoreData {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("StoreData")
            .field("id", &self.id)
            .finish_non_exhaustive()
    }
}
