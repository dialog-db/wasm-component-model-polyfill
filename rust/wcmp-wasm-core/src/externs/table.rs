//! A table.

use crate::checks;
use crate::error::Result;
use crate::internal::{StoreContextInternal, StoreContextMutInternal};
use crate::store::{AsContext, AsContextMut};
use crate::types::TableType;
use crate::values::Val;

handle! {
    /// A table of references.
    ///
    /// An index is a 64-bit number, so a table addressed with 64-bit
    /// numbers uses the same methods. An index outside the table is
    /// [`Error::TableOutOfBounds`](crate::Error::TableOutOfBounds).
    Table
}

impl Table {
    /// A table of type `ty` in `store` whose every element is `init`.
    ///
    /// A table addressed with 64-bit numbers needs
    /// [`memory64`](crate::Capability::Memory64), and an element type can
    /// need a capability of its own. Where the backend lacks the
    /// capability, this is [`Error::Unsupported`](crate::Error::Unsupported).
    pub fn new(mut store: impl AsContextMut, ty: TableType, init: Val) -> Result<Self> {
        let mut store = store.as_context_mut();
        checks::table_type(store.engine().capabilities(), &ty)?;
        let backend = store.backend_mut();
        checks::value_in_store(backend, &init)?;
        backend.table_new(ty, init)
    }

    /// The type of the table.
    pub fn ty(&self, store: impl AsContext) -> Result<TableType> {
        let backend = store.as_context().backend();
        checks::same_store(backend, *self)?;
        backend.table_ty(*self)
    }

    /// The number of elements of the table.
    pub fn size(&self, store: impl AsContext) -> Result<u64> {
        let backend = store.as_context().backend();
        checks::same_store(backend, *self)?;
        backend.table_size(*self)
    }

    /// The element of the table at `index`.
    pub fn get(&self, mut store: impl AsContextMut, index: u64) -> Result<Val> {
        let mut store = store.as_context_mut();
        let backend = store.backend_mut();
        checks::same_store(backend, *self)?;
        backend.table_get(*self, index)
    }

    /// Sets the element of the table at `index` to `value`.
    pub fn set(&self, mut store: impl AsContextMut, index: u64, value: Val) -> Result<()> {
        let mut store = store.as_context_mut();
        let backend = store.backend_mut();
        checks::same_store(backend, *self)?;
        checks::value_in_store(backend, &value)?;
        backend.table_set(*self, index, value)
    }

    /// Grows the table by `delta` elements that are each `init`, and
    /// returns its old size. A growth past the maximum of the table is
    /// [`Error::Grow`](crate::Error::Grow).
    pub fn grow(&self, mut store: impl AsContextMut, delta: u64, init: Val) -> Result<u64> {
        let mut store = store.as_context_mut();
        let backend = store.backend_mut();
        checks::same_store(backend, *self)?;
        checks::value_in_store(backend, &init)?;
        backend.table_grow(*self, delta, init)
    }
}
