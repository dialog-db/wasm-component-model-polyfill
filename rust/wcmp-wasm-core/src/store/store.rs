//! The owner of guest state.

use core::fmt;
use core::marker::PhantomData;

use crate::contract::{BackendStore, MaybeSend};
use crate::engine::Engine;
use crate::error::Result;
use crate::internal::{
    EngineInternal, StoreContextInternal, StoreContextMutInternal, StoreDataInternal,
};
use crate::store::{AsContext, AsContextMut, StoreContext, StoreContextMut, StoreData};

/// The owner of every instance and every object an instance makes, and of
/// the host's data, a `T`.
///
/// Every handle names one store, and means nothing to another. When a store
/// drops, everything it owns drops with it, including each call that waits
/// in it for a resumption.
pub struct Store<T> {
    inner: Box<dyn BackendStore>,
    data: PhantomData<fn() -> T>,
}

impl<T: MaybeSend + 'static> Store<T> {
    /// A store of `engine` that owns `data`.
    ///
    /// Natively `T` must be `Send`, as it must be for Wasmtime's
    /// asynchronous API, because every compile and instantiation is
    /// asynchronous.
    pub fn new(engine: &Engine, data: T) -> Result<Self> {
        let data = StoreData::new(engine.clone(), data);
        let inner = engine.backend().new_store(data)?;
        Ok(Self {
            inner,
            data: PhantomData,
        })
    }
}

impl<T: 'static> Store<T> {
    /// The host's data in the store.
    pub fn data(&self) -> &T {
        self.inner.data().user::<T>()
    }

    /// The host's data in the store, mutably.
    pub fn data_mut(&mut self) -> &mut T {
        self.inner.data_mut().user_mut::<T>()
    }

    /// The engine of the store.
    pub fn engine(&self) -> &Engine {
        self.inner.data().engine()
    }
}

impl<T: 'static> AsContext for Store<T> {
    type Data = T;

    fn as_context(&self) -> StoreContext<'_, T> {
        StoreContext::from_backend(&*self.inner)
    }
}

impl<T: 'static> AsContextMut for Store<T> {
    fn as_context_mut(&mut self) -> StoreContextMut<'_, T> {
        StoreContextMut::from_backend(&mut *self.inner)
    }
}

impl<T> fmt::Debug for Store<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Store")
            .field("id", &self.inner.data().id())
            .finish_non_exhaustive()
    }
}
