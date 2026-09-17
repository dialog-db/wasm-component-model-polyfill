//! Resource-handle support: identity, per-store handle tables, and
//! the host-resource registration carrier.
//!
//! The polyfill realises the canonical-ABI's runtime-state rules for
//! `own<T>` and `borrow<T>` against the types in this module:
//!
//! - [`ResourceTypeId`] is the engine-issued identity that names a
//!   registered resource type. Every host registration mints one.
//! - [`HandleTable`] is one slab the canonical ABI's
//!   index-allocation and reuse rules govern, and [`HandleTables`]
//!   is the store's collection of them.
//! - [`ResourceHandle`] is the polyfill's opaque addressing surface
//!   for handles that pass through [`Val::Own`] and [`Val::Borrow`].
//!
//! Host-resource registration itself lives on
//! [`LinkerInstance`]; the carrier type a [`Linker`] stores per
//! interface lives in [`super::linker`].
//!
//! [`Linker`]: crate::Linker
//! [`LinkerInstance`]: crate::LinkerInstance
//! [`Val::Own`]: crate::Val::Own
//! [`Val::Borrow`]: crate::Val::Borrow

mod handle;
mod handle_kind;
mod handle_lookup_error;
mod identity;
mod table;
mod table_id;
mod table_runtime;
mod tables;

pub use handle::ResourceHandle;
pub use handle_kind::HandleKind;
pub use handle_lookup_error::HandleLookupError;
pub use identity::ResourceTypeId;
pub use table_id::TableId;
pub use table_runtime::ResourceTableRuntime;
pub use tables::HandleTables;
