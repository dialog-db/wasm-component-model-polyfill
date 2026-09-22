//! The parts a [`ResourceHandle`] is built from.
//!
//! [`ResourceHandle`] is re-exported by `lib.rs` and its parts are
//! not: a handle names a live entry in one of a store's handle
//! tables, so a handle safe host code assembled for itself would
//! address an entry the store never gave it. The fields are private
//! for that reason, and this is what the crate builds one from. The
//! type is never re-exported, so the [`From`] impl that takes it is
//! an entry only the crate can reach.
//!
//! [`ResourceHandle`]: super::ResourceHandle

use super::identity::ResourceTypeId;

/// The parts a [`ResourceHandle`] is built from.
///
/// [`ResourceHandle`]: super::ResourceHandle
pub struct ResourceHandleParts {
    /// The engine-issued identity of the resource type the handle
    /// addresses.
    pub type_id: ResourceTypeId,
    /// The handle-table entry the handle names.
    pub index: u32,
    /// The resource's 32-bit representation.
    pub rep: u32,
}
