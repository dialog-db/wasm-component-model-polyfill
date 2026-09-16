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
use super::handle_kind::HandleKind;
use super::handle_lookup_error::HandleLookupError;
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
            if let Some(HandleKind::Own { lend_count, .. }) =
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
            Some(HandleKind::Own { lend_count, .. }) => {
                *lend_count += 1;
                self.scopes[scope].lenders.push((table, index));
                true
            }
            _ => false,
        }
    }

    /// Insert an owning entry for `rep` of resource type `type_id`
    /// into `table` and return its index.
    pub fn insert_own(
        &mut self,
        table: TableId,
        type_id: ResourceTypeId,
        guest_defined: bool,
        rep: u32,
    ) -> u32 {
        self.for_table_mut(table).insert_entry(HandleKind::Own {
            type_id,
            guest_defined,
            rep,
            lend_count: 0,
        })
    }

    /// Insert a borrow of `rep` of resource type `type_id` into
    /// `table`, owed to the current call. Returns the new index, or
    /// `None` when no call is in flight.
    pub fn insert_borrow(
        &mut self,
        table: TableId,
        type_id: ResourceTypeId,
        guest_defined: bool,
        rep: u32,
    ) -> Option<u32> {
        let scope = self.current_scope()?;
        self.scopes[scope].borrow_count += 1;
        Some(self.for_table_mut(table).insert_entry(HandleKind::Borrow {
            type_id,
            guest_defined,
            rep,
            scope,
        }))
    }

    /// Insert a subtask entry that points at index `subtask` of the
    /// store's subtask table, and return the handle-table index.
    pub fn insert_subtask(&mut self, table: TableId, subtask: u32) -> u32 {
        self.for_table_mut(table)
            .insert_entry(HandleKind::Subtask { index: subtask })
    }

    /// Insert a waitable-set entry that points at index `set` of the
    /// store's waitable-set table, and return the handle-table index.
    pub fn insert_waitable_set(&mut self, table: TableId, set: u32) -> u32 {
        self.for_table_mut(table)
            .insert_entry(HandleKind::WaitableSet { index: set })
    }

    /// Read the entry at `index` of `table`, of any kind, with no
    /// type check. Used for a handle kind that carries no resource
    /// type, such as a subtask or a waitable set.
    pub fn entry(&self, table: TableId, index: u32) -> Option<HandleKind> {
        self.for_table(table).and_then(|t| t.entry(index)).copied()
    }

    /// Remove the entry at `index` of `table`, of any kind, with no
    /// type check and no ownership or borrow bookkeeping. A resource
    /// entry is removed through [`remove_own`](Self::remove_own)
    /// instead, which enforces that bookkeeping.
    pub fn remove(&mut self, table: TableId, index: u32) -> Option<HandleKind> {
        self.for_table_mut(table).remove(index)
    }

    /// Read the entry at `index` of `table`, checking that it holds a
    /// resource of type `type_id`. A component instance keeps one
    /// table for every handle kind it uses, so an index of one
    /// resource type can name an entry of another type, or an entry
    /// that is not a resource at all; both are the wrong-type trap
    /// and the wrong-kind failure respectively.
    pub fn lookup(
        &self,
        table: TableId,
        index: u32,
        type_id: ResourceTypeId,
        guest_defined: bool,
    ) -> Result<HandleKind, HandleLookupError> {
        let entry = self
            .for_table(table)
            .and_then(|t| t.entry(index))
            .copied()
            .ok_or(HandleLookupError::Unknown { index })?;
        let (found_type, found_guest) = match entry {
            HandleKind::Own {
                type_id,
                guest_defined,
                ..
            }
            | HandleKind::Borrow {
                type_id,
                guest_defined,
                ..
            } => (type_id, guest_defined),
            _ => return Err(HandleLookupError::WrongKind { index }),
        };
        if found_type != type_id {
            return Err(HandleLookupError::WrongType {
                index,
                expected_guest: guest_defined,
                found_guest,
            });
        }
        Ok(entry)
    }

    /// Remove the owning entry at `index` of `table` and return its
    /// rep. The entry must hold a resource of type `type_id`, must
    /// own it, and must not be lent out as a borrow.
    pub fn remove_own(
        &mut self,
        table: TableId,
        index: u32,
        type_id: ResourceTypeId,
        guest_defined: bool,
    ) -> Result<u32, HandleLookupError> {
        let entry = self.lookup(table, index, type_id, guest_defined)?;
        match entry {
            HandleKind::Own {
                lend_count: 0, rep, ..
            } => {
                self.for_table_mut(table).remove(index);
                Ok(rep)
            }
            HandleKind::Own { .. } => Err(HandleLookupError::Lent),
            HandleKind::Borrow { .. } => Err(HandleLookupError::NotOwned { index }),
            _ => unreachable!("lookup only ever returns a resource entry"),
        }
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
}

impl Default for HandleTables {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn it_rejects_an_index_of_another_resource_type() {
        let mut tables = HandleTables::new();
        let table = TableId::fresh();
        let first = ResourceTypeId::fresh();
        let second = ResourceTypeId::fresh();
        let index = tables.insert_own(table, first, true, 7);
        assert_eq!(
            tables.lookup(table, index, second, true),
            Err(HandleLookupError::WrongType {
                index,
                expected_guest: true,
                found_guest: true,
            })
        );
        assert_eq!(
            tables
                .lookup(table, index, second, true)
                .unwrap_err()
                .to_string(),
            "handle index 1 used with the wrong type, expected guest-defined resource but found a different guest-defined resource"
        );
        assert_eq!(
            tables.lookup(table, index, first, true).unwrap().rep(),
            Some(7)
        );
    }

    #[test]
    fn it_allocates_consecutive_indices_across_resource_types_in_one_table() {
        let mut tables = HandleTables::new();
        let table = TableId::fresh();
        let first = ResourceTypeId::fresh();
        let second = ResourceTypeId::fresh();
        let a = tables.insert_own(table, first, true, 1);
        let b = tables.insert_own(table, second, true, 2);
        assert_eq!(b, a + 1, "one allocator serves every resource type");
    }

    #[test]
    fn it_inserts_looks_up_and_removes_a_subtask_and_a_waitable_set_entry() {
        let mut tables = HandleTables::new();
        let table = TableId::fresh();
        let subtask_index = tables.insert_subtask(table, 5);
        let set_index = tables.insert_waitable_set(table, 9);
        assert_ne!(subtask_index, set_index);
        assert_eq!(
            tables.entry(table, subtask_index),
            Some(HandleKind::Subtask { index: 5 })
        );
        assert_eq!(
            tables.entry(table, set_index),
            Some(HandleKind::WaitableSet { index: 9 })
        );

        let ty = ResourceTypeId::fresh();
        assert_eq!(
            tables.lookup(table, subtask_index, ty, true),
            Err(HandleLookupError::WrongKind {
                index: subtask_index
            })
        );
        assert_eq!(
            tables.lookup(table, set_index, ty, true),
            Err(HandleLookupError::WrongKind { index: set_index })
        );

        assert_eq!(
            tables.remove(table, subtask_index),
            Some(HandleKind::Subtask { index: 5 })
        );
        assert_eq!(
            tables.remove(table, set_index),
            Some(HandleKind::WaitableSet { index: 9 })
        );
        assert_eq!(tables.entry(table, subtask_index), None);
        assert_eq!(tables.entry(table, set_index), None);
    }

    #[test]
    fn it_refuses_to_remove_a_lent_entry_until_the_call_ends() {
        let mut tables = HandleTables::new();
        let table = TableId::fresh();
        let ty = ResourceTypeId::fresh();
        let index = tables.insert_own(table, ty, false, 3);
        tables.enter_call();
        assert!(tables.lend(table, index));
        assert_eq!(
            tables.remove_own(table, index, ty, false),
            Err(HandleLookupError::Lent)
        );
        assert_eq!(tables.exit_call(), Ok(()));
        assert_eq!(tables.remove_own(table, index, ty, false), Ok(3));
        assert_eq!(
            tables.remove_own(table, index, ty, false),
            Err(HandleLookupError::Unknown { index })
        );
    }
}
