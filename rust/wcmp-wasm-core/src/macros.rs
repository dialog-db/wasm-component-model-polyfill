//! The one shape every handle to an object of a store shares.

/// Defines a handle: a copyable pair of the store that owns an object and
/// the index the backend gave the object in that store.
///
/// Wasmtime's handles have the same shape. A handle is plain data. It holds
/// nothing alive, and it means something only to the store it names.
macro_rules! handle {
    ($(#[$attribute:meta])* $name:ident) => {
        $(#[$attribute])*
        #[derive(Clone, Copy, Debug)]
        pub struct $name {
            store: $crate::store::StoreId,
            index: u64,
        }

        impl $crate::contract::RawHandle for $name {
            fn from_raw(store: $crate::store::StoreId, index: u64) -> Self {
                Self { store, index }
            }

            fn store_id(&self) -> $crate::store::StoreId {
                self.store
            }

            fn index(&self) -> u64 {
                self.index
            }
        }
    };
}
