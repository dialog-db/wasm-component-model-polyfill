//! How a backend makes and reads the handles of its store.

use crate::store::StoreId;

/// A handle to an object of a store, as a backend makes and reads it.
///
/// Every handle type of this crate implements the trait: [`Instance`],
/// [`Func`], [`Memory`], [`Global`], [`Table`], [`Tag`], [`ExternRef`],
/// [`AnyRef`], [`ExnRef`], and [`ContRef`]. A handle is the [`StoreId`] of
/// the store that owns the object, and an index that the backend chose for
/// the object in that store.
///
/// [`Instance`]: crate::Instance
/// [`Func`]: crate::Func
/// [`Memory`]: crate::Memory
/// [`Global`]: crate::Global
/// [`Table`]: crate::Table
/// [`Tag`]: crate::Tag
/// [`ExternRef`]: crate::ExternRef
/// [`AnyRef`]: crate::AnyRef
/// [`ExnRef`]: crate::ExnRef
/// [`ContRef`]: crate::ContRef
pub trait RawHandle: Copy {
    /// Makes a handle to the object at `index` in the store `store`.
    fn from_raw(store: StoreId, index: u64) -> Self;

    /// The store that owns the object.
    fn store_id(&self) -> StoreId;

    /// The index the backend gave the object in its store.
    fn index(&self) -> u64;
}
