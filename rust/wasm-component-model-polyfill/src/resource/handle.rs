//! The polyfill's opaque handle into one of a store's handle tables.
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

use super::handle_parts::ResourceHandleParts;
use super::identity::ResourceTypeId;

/// An opaque handle into one of a store's handle tables.
///
/// The polyfill mints these when a host registers a resource against
/// a [`LinkerInstance`] and hands one across the canonical-ABI
/// boundary, or when a guest produces one during a lifted call. A
/// handle's index is meaningful only against the table it names: the
/// per-instance table of the instance the handle came from, or, for
/// an `own<T>` handle the host holds outright, the host's
/// per-resource-type table in the [`Store`].
///
/// The parts are readable and not writable. A handle names a live
/// entry in a table the store owns, so only the store mints one: it
/// is built from `ResourceHandleParts`, which `lib.rs` never
/// re-exports, and host code that assembled a handle of its own
/// would address an entry the store never gave it. A host reads the
/// parts through [`Self::type_id`], [`Self::index`], and
/// [`Self::rep`], and writes none of them:
///
/// ```rust
/// use wasm_component_model_polyfill::ResourceHandle;
/// fn parts(handle: &ResourceHandle) -> (u32, u32) {
///     (handle.index(), handle.rep())
/// }
/// ```
///
/// ```compile_fail
/// # use wasm_component_model_polyfill::{Engine, ResourceHandle, ResourceTypeId, Store};
/// # fn forge(type_id: ResourceTypeId) -> ResourceHandle {
/// ResourceHandle {
///     type_id,
///     index: 999,
///     rep: 0,
/// }
/// # }
/// ```
///
/// The parts type is the same rule stated once more, for the entry
/// that builds a handle rather than for the fields. The crate builds
/// one through [`From<ResourceHandleParts>`][From], and `lib.rs`
/// re-exports the handle and not its parts, so naming the parts at
/// all is what fails — not the conversion, which is a perfectly
/// ordinary `impl`:
///
/// ```compile_fail
/// use wasm_component_model_polyfill::{ResourceHandle, ResourceHandleParts};
/// fn forge(parts: ResourceHandleParts) -> ResourceHandle {
///     parts.into()
/// }
/// ```
///
/// [`LinkerInstance`]: crate::LinkerInstance
/// [`Store`]: crate::Store
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct ResourceHandle {
    /// The engine-issued identity of the resource type this handle
    /// addresses.
    type_id: ResourceTypeId,
    /// The handle-table entry this handle names.
    index: u32,
    /// The resource's 32-bit representation. Carried so that a
    /// handle the host owns outright (one lifted out of a guest as
    /// `own<T>`, whose table entry the lift removed) can be lowered
    /// back into a guest, which re-inserts the rep and takes a fresh
    /// index.
    rep: u32,
}

impl ResourceHandle {
    /// The identity of the resource type this handle addresses.
    pub fn type_id(&self) -> ResourceTypeId {
        self.type_id
    }

    /// The handle-table entry this handle names.
    ///
    /// The index is meaningful only against the table the handle
    /// came from, which the handle itself does not name.
    pub fn index(&self) -> u32 {
        self.index
    }

    /// The resource's 32-bit representation: what the host supplied
    /// when it minted the handle, or what a lift read out of the
    /// guest's table entry.
    pub fn rep(&self) -> u32 {
        self.rep
    }
}

impl From<ResourceHandleParts> for ResourceHandle {
    fn from(parts: ResourceHandleParts) -> Self {
        Self {
            type_id: parts.type_id,
            index: parts.index,
            rep: parts.rep,
        }
    }
}
