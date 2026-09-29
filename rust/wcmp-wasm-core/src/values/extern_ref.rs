//! A reference the host made.

use core::any::Any;

use crate::checks;
use crate::error::Result;
use crate::internal::{StoreContextInternal, StoreContextMutInternal};
use crate::store::{AsContext, AsContextMut};

handle! {
    /// An `externref`: a reference to a value the host made, which a guest
    /// can hold and hand back, and the host can read.
    ExternRef
}

impl ExternRef {
    /// An `externref` in `store` that holds `value`.
    pub fn new(mut store: impl AsContextMut, value: impl Any + Send + Sync) -> Result<Self> {
        store
            .as_context_mut()
            .backend_mut()
            .extern_ref_new(Box::new(value))
    }

    /// The value the reference holds.
    pub fn data<'a>(&self, store: &'a impl AsContext) -> Result<&'a (dyn Any + Send + Sync)> {
        let store = store.as_context().backend();
        checks::same_store(store, *self)?;
        store.extern_ref_data(*self)
    }
}
