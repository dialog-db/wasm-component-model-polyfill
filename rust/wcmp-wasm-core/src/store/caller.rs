//! The context a host function receives.

use core::fmt;
use core::marker::PhantomData;

use crate::contract::BackendStore;
use crate::engine::Engine;
use crate::internal::{
    CallerInternal, StoreContextInternal, StoreContextMutInternal, StoreDataInternal,
};
use crate::store::{AsContext, AsContextMut, StoreContext, StoreContextMut};

/// The context a host function receives: the store the calling guest runs
/// in, as Wasmtime's `Caller` is.
///
/// Through it, the host function reads and changes the host's data, reads
/// and writes guest memory, and calls back into the guest, at any depth.
pub struct Caller<'a, T> {
    store: &'a mut dyn BackendStore,
    data: PhantomData<fn() -> T>,
}

impl<T: 'static> Caller<'_, T> {
    /// The host's data in the store.
    pub fn data(&self) -> &T {
        self.store.data().user::<T>()
    }

    /// The host's data in the store, mutably.
    pub fn data_mut(&mut self) -> &mut T {
        self.store.data_mut().user_mut::<T>()
    }

    /// The engine of the store.
    pub fn engine(&self) -> &Engine {
        self.store.engine()
    }
}

impl<'a, T> CallerInternal<'a> for Caller<'a, T> {
    fn from_backend(store: &'a mut dyn BackendStore) -> Self {
        Self {
            store,
            data: PhantomData,
        }
    }
}

impl<T: 'static> AsContext for Caller<'_, T> {
    type Data = T;

    fn as_context(&self) -> StoreContext<'_, T> {
        StoreContext::from_backend(&*self.store)
    }
}

impl<T: 'static> AsContextMut for Caller<'_, T> {
    fn as_context_mut(&mut self) -> StoreContextMut<'_, T> {
        StoreContextMut::from_backend(&mut *self.store)
    }
}

impl<T> fmt::Debug for Caller<'_, T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Caller")
            .field("store", &self.store.id())
            .finish_non_exhaustive()
    }
}
