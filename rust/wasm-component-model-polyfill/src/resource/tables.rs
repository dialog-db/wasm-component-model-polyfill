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

use super::call_scope::CallScope;
use super::handle_entry::HandleEntry;
use super::identity::ResourceTypeId;
use super::table::HandleTable;

/// The full set of per-resource-type handle tables a [`Store`]
/// carries.
///
/// [`Store`]: crate::Store
pub struct HandleTables {
    tables: HashMap<ResourceTypeId, HandleTable>,
    /// The store's call stack: one scope per call in flight across
    /// the host boundary, innermost last.
    scopes: Vec<CallScope>,
}

impl HandleTables {
    /// Construct an empty collection.
    pub fn new() -> Self {
        Self {
            tables: HashMap::new(),
            scopes: Vec::new(),
        }
    }

    /// Push a call scope: a call is crossing the host boundary.
    pub fn enter_call(&mut self) {
        self.scopes.push(CallScope::default());
    }

    /// The position of the innermost call scope, if a call is in
    /// flight.
    pub fn current_scope(&self) -> Option<usize> {
        self.scopes.len().checked_sub(1)
    }

    /// End the innermost call on its success path: every borrow
    /// lowered into the guest during the call must have been dropped,
    /// else the residual count is returned and the scope is still
    /// popped. Each lend recorded during the call is undone.
    pub fn exit_call(&mut self) -> Result<(), u32> {
        let Some(scope) = self.scopes.pop() else {
            return Ok(());
        };
        for (type_id, index) in scope.lenders {
            if let Some(HandleEntry::Own { lend_count, .. }) =
                self.for_type_mut(type_id).entry_mut(index)
            {
                *lend_count = lend_count.saturating_sub(1);
            }
        }
        if scope.borrow_count > 0 {
            Err(scope.borrow_count)
        } else {
            Ok(())
        }
    }

    /// End the innermost call on its failure path: the scope is
    /// popped and its lends undone, and no borrow check is made,
    /// because the call already failed.
    pub fn abandon_call(&mut self) {
        let _ = self.exit_call();
    }

    /// Record that a borrow of the owning entry `(type_id, index)`
    /// was lifted into the host during the current call, so the
    /// entry cannot be removed until the call ends. Returns `false`
    /// when the entry is not an owning entry.
    pub fn lend(&mut self, type_id: ResourceTypeId, index: u32) -> bool {
        let Some(scope) = self.current_scope() else {
            return false;
        };
        match self.for_type_mut(type_id).entry_mut(index) {
            Some(HandleEntry::Own { lend_count, .. }) => {
                *lend_count += 1;
                self.scopes[scope].lenders.push((type_id, index));
                true
            }
            _ => false,
        }
    }

    /// Insert a borrow of `rep` into the table for `type_id`, owed to
    /// the current call. Returns the new index, or `None` when no
    /// call is in flight.
    pub fn insert_borrow(&mut self, type_id: ResourceTypeId, rep: u32) -> Option<u32> {
        let scope = self.current_scope()?;
        self.scopes[scope].borrow_count += 1;
        Some(self.for_type_mut(type_id).insert_borrow(rep, scope))
    }

    /// Drop a borrow entry: the guest returned the handle it was
    /// lent. Returns `false` when the scope the borrow belongs to is
    /// no longer on the stack.
    pub fn return_borrow(&mut self, scope: usize) -> bool {
        match self.scopes.get_mut(scope) {
            Some(call) => {
                call.borrow_count = call.borrow_count.saturating_sub(1);
                true
            }
            None => false,
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
