//! Per-store collection of handle tables, keyed by resource-type
//! identity.
//!
//! Each [`Store`] owns one `HandleTables` instance. The collection
//! lazily creates a [`HandleTable`] the first time a given
//! [`ResourceTypeId`] is touched, so resource types that go
//! unexercised cost nothing.
//!
//! Workspace-internal: the collection is reached only through
//! crate-private accessors on [`Store`]. The public API exposes the
//! handles themselves through the [`Val`] family rather than the
//! tables.
//!
//! [`Store`]: crate::Store
//! [`Val`]: crate::Val

use std::collections::HashMap;

use super::table::HandleTable;
use super::identity::ResourceTypeId;

/// The full set of per-resource-type handle tables a [`Store`]
/// carries.
///
/// [`Store`]: crate::Store
pub struct HandleTables {
    tables: HashMap<ResourceTypeId, HandleTable>,
}

impl HandleTables {
    /// Construct an empty collection.
    pub fn new() -> Self {
        Self {
            tables: HashMap::new(),
        }
    }

    /// Borrow the table for the given resource-type identity,
    /// creating it on first access.
    pub fn for_type_mut(&mut self, type_id: ResourceTypeId) -> &mut HandleTable {
        self.tables.entry(type_id).or_default()
    }

    /// Borrow the table for the given resource-type identity, or
    /// `None` if no entry has been allocated for it yet.
    pub fn for_type(&self, type_id: ResourceTypeId) -> Option<&HandleTable> {
        self.tables.get(&type_id)
    }

    /// Iterate over `(type_id, table)` pairs in the collection.
    /// Iteration order is unspecified; consumers that need to look
    /// up a specific entry should use [`Self::for_type`] instead.
    pub fn iter(&self) -> impl Iterator<Item = (ResourceTypeId, &HandleTable)> {
        self.tables.iter().map(|(id, table)| (*id, table))
    }
}

impl Default for HandleTables {
    fn default() -> Self {
        Self::new()
    }
}
