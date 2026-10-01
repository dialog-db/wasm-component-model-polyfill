//! Write access to a store.

use core::fmt;
use core::marker::PhantomData;

use crate::contract::BackendStore;
use crate::engine::Engine;
use crate::internal::{StoreContextInternal, StoreContextMutInternal, StoreDataInternal};
use crate::store::{AsContext, AsContextMut, StoreContext};

/// Write access to a store whose host data is a `T`.
pub struct StoreContextMut<'a, T> {
    store: &'a mut dyn BackendStore,
    data: PhantomData<fn() -> T>,
}

impl<T: 'static> StoreContextMut<'_, T> {
    /// The host's data in the store.
    ///
    /// # Panics
    ///
    /// Where [`Store::data`](crate::Store::data) does, for a context made
    /// from a store.
    pub fn data(&self) -> &T {
        self.store.data().user::<T>()
    }

    /// The host's data in the store, mutably.
    ///
    /// # Panics
    ///
    /// Where [`Store::data`](crate::Store::data) does, for a context made
    /// from a store.
    pub fn data_mut(&mut self) -> &mut T {
        self.store.data_mut().user_mut::<T>()
    }

    /// The engine of the store.
    pub fn engine(&self) -> &Engine {
        self.store.engine()
    }
}

impl<'a, T> StoreContextMutInternal<'a> for StoreContextMut<'a, T> {
    fn from_backend(store: &'a mut dyn BackendStore) -> Self {
        Self {
            store,
            data: PhantomData,
        }
    }

    fn backend_mut(&mut self) -> &mut dyn BackendStore {
        &mut *self.store
    }
}

impl<T: 'static> AsContext for StoreContextMut<'_, T> {
    type Data = T;

    fn as_context(&self) -> StoreContext<'_, T> {
        StoreContext::from_backend(&*self.store)
    }
}

impl<T: 'static> AsContextMut for StoreContextMut<'_, T> {
    fn as_context_mut(&mut self) -> StoreContextMut<'_, T> {
        StoreContextMut::from_backend(&mut *self.store)
    }
}

impl<T> fmt::Debug for StoreContextMut<'_, T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("StoreContextMut")
            .field("id", &self.store.id())
            .finish_non_exhaustive()
    }
}
