//! The polyfill's opaque handle into a per-store resource table.
//!
//! A `ResourceHandle` is the value the polyfill exposes through the
//! [`Val::Own`] and [`Val::Borrow`] variants. It carries the
//! resource-type identity the handle was minted against alongside
//! the handle-table index that names the live entry.
//!
//! Identity comparison is structural: two handles compare equal when
//! both their resource-type and index components match. A handle is
//! cheaply cloneable; copying does not duplicate the underlying
//! table entry.
//!
//! [`Val::Own`]: crate::Val::Own
//! [`Val::Borrow`]: crate::Val::Borrow

use super::identity::ResourceTypeId;

/// An opaque handle into a per-store resource table.
///
/// The polyfill mints these when a host registers a resource against
/// a [`LinkerInstance`] and hands one across the canonical-ABI
/// boundary, or when a guest produces one during a lifted call. A
/// handle's index is meaningful only against the table it names: the
/// per-instance table of the instance the handle came from, or, for
/// an `own<T>` handle the host holds outright, the host's
/// per-resource-type table in the [`Store`].
///
/// [`LinkerInstance`]: crate::LinkerInstance
/// [`Store`]: crate::Store
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct ResourceHandle {
    /// The engine-issued identity of the resource type this handle
    /// addresses. Workspace-internal; consumers see the handle as
    /// opaque.
    pub type_id: ResourceTypeId,
    /// The handle-table entry this handle names. Workspace-internal;
    /// consumers see the handle as opaque.
    pub index: u32,
    /// The resource's 32-bit representation. Carried so that a
    /// handle the host owns outright (one lifted out of a guest as
    /// `own<T>`, whose table entry the lift removed) can be lowered
    /// back into a guest, which re-inserts the rep and takes a fresh
    /// index. Workspace-internal; consumers see the handle as opaque.
    pub rep: u32,
}
