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
use super::table_id::TableId;

/// Every handle table a [`Store`] carries, with the call stack the
/// canonical ABI keeps for borrows.
///
/// [`Store`]: crate::Store
pub struct HandleTables {
    /// Every table in the store, by identity: one per component
    /// instance per resource type, plus the host's per type.
    tables: HashMap<TableId, HandleTable>,
    /// The host's table for each resource type, created on first use.
    host_tables: HashMap<ResourceTypeId, TableId>,
    /// The store's call stack: one scope per call in flight across
    /// the host boundary, innermost last.
    scopes: Vec<CallScope>,
}

impl HandleTables {
    /// Construct an empty collection.
    pub fn new() -> Self {
        Self {
            tables: HashMap::new(),
            host_tables: HashMap::new(),
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
        for (table, index) in scope.lenders {
            if let Some(HandleEntry::Own { lend_count, .. }) =
                self.for_table_mut(table).entry_mut(index)
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

    /// Record that a borrow of the owning entry `(table, index)` was
    /// lifted out during the current call, so the entry cannot be
    /// removed until the call ends. Returns `false` when the entry is
    /// not an owning entry or no call is in flight.
    pub fn lend(&mut self, table: TableId, index: u32) -> bool {
        let Some(scope) = self.current_scope() else {
            return false;
        };
        match self.for_table_mut(table).entry_mut(index) {
            Some(HandleEntry::Own { lend_count, .. }) => {
                *lend_count += 1;
                self.scopes[scope].lenders.push((table, index));
                true
            }
            _ => false,
        }
    }

    /// Insert a borrow of `rep` into `table`, owed to the current
    /// call. Returns the new index, or `None` when no call is in
    /// flight.
    pub fn insert_borrow(&mut self, table: TableId, rep: u32) -> Option<u32> {
        let scope = self.current_scope()?;
        self.scopes[scope].borrow_count += 1;
        Some(self.for_table_mut(table).insert_borrow(rep, scope))
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

    /// Borrow the table with the given identity, creating it on first
    /// access.
    pub fn for_table_mut(&mut self, table: TableId) -> &mut HandleTable {
        self.tables.entry(table).or_default()
    }

    /// Borrow the table with the given identity, or `None` if nothing
    /// has been allocated in it yet.
    pub fn for_table(&self, table: TableId) -> Option<&HandleTable> {
        self.tables.get(&table)
    }

    /// The host's table for a resource type, created on first access.
    /// Handles the host holds (`Store::resource_new`, an `own<T>`
    /// lifted out of a guest) live here.
    pub fn host_table(&mut self, type_id: ResourceTypeId) -> TableId {
        *self
            .host_tables
            .entry(type_id)
            .or_insert_with(TableId::fresh)
    }

    /// Borrow the host's table for a resource type, creating it on
    /// first access.
    pub fn for_type_mut(&mut self, type_id: ResourceTypeId) -> &mut HandleTable {
        let table = self.host_table(type_id);
        self.for_table_mut(table)
    }

    /// Borrow the host's table for a resource type, or `None` if the
    /// host holds nothing of that type yet.
    pub fn for_type(&self, type_id: ResourceTypeId) -> Option<&HandleTable> {
        self.host_tables
            .get(&type_id)
            .and_then(|table| self.tables.get(table))
    }
}

impl Default for HandleTables {
    fn default() -> Self {
        Self::new()
    }
}
