//! A global.

use crate::checks;
use crate::error::Result;
use crate::internal::{StoreContextInternal, StoreContextMutInternal};
use crate::store::{AsContext, AsContextMut};
use crate::types::GlobalType;
use crate::values::Val;

handle! {
    /// A global: one value, which can change where the global is mutable.
    Global
}

impl Global {
    /// A global of type `ty` in `store` that holds `value`.
    ///
    /// A type that needs a capability the backend lacks, such as a GC
    /// reference, is [`Error::Unsupported`](crate::Error::Unsupported).
    pub fn new(mut store: impl AsContextMut, ty: GlobalType, value: Val) -> Result<Self> {
        let mut store = store.as_context_mut();
        checks::val_type(store.engine().capabilities(), ty.content())?;
        let backend = store.backend_mut();
        checks::value_in_store(backend, &value)?;
        backend.global_new(ty, value)
    }

    /// The type of the global.
    pub fn ty(&self, store: impl AsContext) -> Result<GlobalType> {
        let backend = store.as_context().backend();
        checks::same_store(backend, *self)?;
        backend.global_ty(*self)
    }

    /// The value the global holds.
    pub fn get(&self, mut store: impl AsContextMut) -> Result<Val> {
        let mut store = store.as_context_mut();
        let backend = store.backend_mut();
        checks::same_store(backend, *self)?;
        backend.global_get(*self)
    }

    /// Sets the value of the global, which must be mutable.
    pub fn set(&self, mut store: impl AsContextMut, value: Val) -> Result<()> {
        let mut store = store.as_context_mut();
        let backend = store.backend_mut();
        checks::same_store(backend, *self)?;
        checks::value_in_store(backend, &value)?;
        backend.global_set(*self, value)
    }
}
