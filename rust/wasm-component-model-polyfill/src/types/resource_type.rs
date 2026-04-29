//! Identity of a resource type referenced by an `own<T>` or
//! `borrow<T>` handle slot.
//!
//! At this stage of the polyfill, resource types are represented in
//! the introspection surface as named slots. The full handle-table
//! behaviour — destruction, ownership transfer, the canonical-ABI
//! handle index space — is the responsibility of a later layer; this
//! type exists so that a parsed component's imports and exports can
//! be walked and reported even when they reference resources.

/// The identity of a resource type referenced by a handle.
///
/// Two resource types compare equal when their labels match. The
/// label is whatever name the parser assigned the resource — its
/// declared name in the component's local types, or the import path
/// it was introduced at — and is meaningful only within the
/// component it belongs to.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct ResourceType {
    label: String,
}

impl ResourceType {
    /// Construct a resource type identity from a label.
    pub fn new(label: impl Into<String>) -> Self {
        Self {
            label: label.into(),
        }
    }

    /// The resource type's label.
    pub fn label(&self) -> &str {
        &self.label
    }
}
