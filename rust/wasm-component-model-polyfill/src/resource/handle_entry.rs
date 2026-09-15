//! One live entry of a handle table.

use super::handle_kind::HandleKind;
use super::identity::ResourceTypeId;

/// A live handle-table entry: the resource it refers to, the
/// resource's type, and whether the entry owns or borrows it. A
/// component instance keeps one table for all of its resource types,
/// so every access checks the type against the one the caller
/// expects.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HandleEntry {
    /// The resource's 32-bit representation.
    pub rep: u32,
    /// The identity of the resource's type.
    pub type_id: ResourceTypeId,
    /// Whether a component defines the type (`true`) or the host does.
    /// Only the trap message reads it.
    pub guest_defined: bool,
    /// Owned or borrowed.
    pub kind: HandleKind,
}

impl HandleEntry {
    /// The lend count of an owning entry, or `None` for a borrow.
    pub fn lend_count(&self) -> Option<u32> {
        match self.kind {
            HandleKind::Own { lend_count } => Some(lend_count),
            HandleKind::Borrow { .. } => None,
        }
    }

    /// The Wasmtime word for who defines the resource.
    pub fn definer(&self) -> &'static str {
        if self.guest_defined {
            "guest-defined"
        } else {
            "host-defined"
        }
    }
}
