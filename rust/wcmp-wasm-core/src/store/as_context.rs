//! Read access to a store, from whatever holds it.

use crate::store::StoreContext;

/// A type that gives read access to a store: a [`Store`](crate::Store), a
/// [`Caller`](crate::Caller), or a context, or a reference to one.
///
/// Every method that reads a store takes `impl AsContext`, as Wasmtime's
/// methods do.
pub trait AsContext {
    /// The host's data in the store.
    type Data: 'static;

    /// Read access to the store.
    fn as_context(&self) -> StoreContext<'_, Self::Data>;
}

impl<C: AsContext + ?Sized> AsContext for &C {
    type Data = C::Data;

    fn as_context(&self) -> StoreContext<'_, Self::Data> {
        C::as_context(*self)
    }
}

impl<C: AsContext + ?Sized> AsContext for &mut C {
    type Data = C::Data;

    fn as_context(&self) -> StoreContext<'_, Self::Data> {
        C::as_context(*self)
    }
}
