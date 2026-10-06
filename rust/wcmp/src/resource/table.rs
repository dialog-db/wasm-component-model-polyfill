// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! One handle table.
//!
//! A `HandleTable` is the slab the canonical ABI's runtime-state
//! rules require: the reference's `handles` table on a
//! `ComponentInstance`. The polyfill keeps one table per component
//! instance, shared by every resource type and every other handle
//! kind the instance uses, plus one table per resource type for the
//! handles the host owns outright, all reached through crate-private
//! accessors on the store.
//!
//! Allocation policy follows the canonical-ABI runtime-state rules:
//!
//! - Index 0 is reserved and never handed out, as in the canonical
//!   ABI's reference definitions and in Wasmtime, so the first
//!   allocation in a fresh table is index 1.
//! - Indices are non-aliasing while live: every minted index points
//!   to a distinct table entry.
//! - Reuse of a freed index is deterministic within one instance
//!   table: a free list of that table's freed indices is drained
//!   LIFO so reuse is fixed by the order of drops.
//!
//! Borrows live in the table too: a `borrow<T>` lowered into the
//! guest is an entry owed to the call it was lowered in, and an
//! owning entry a borrow was lifted out of counts its lends, which
//! is what the collection's owned-removal path reads to refuse the
//! removal before the call the borrow was lent to takes delivery of
//! its result. A borrow entry counts no lends of its own, for the
//! reason `HandleTables::lend_to` gives.
//!
//! [`Store`]: crate::Store

use super::handle_kind::HandleKind;

/// One entry in the slab.
///
/// Free entries form a singly-linked list; occupied entries carry
/// the handle's kind, whatever it is.
enum Slot {
    /// Index 0, which the canonical ABI never hands out: a handle of
    /// 0 is never valid, so the first allocation is index 1.
    Reserved,
    Free {
        /// Index of the next free slot, or `None` if this is the
        /// list tail.
        next: Option<u32>,
        /// The generation the slot's next entry takes.
        generation: u32,
    },
    Occupied {
        /// The entry's kind.
        entry: HandleKind,
        /// How many entries the slot held before this one, so that a
        /// handle that recorded an earlier one can be told from it.
        generation: u32,
    },
}

/// One handle table, owned by a single [`Store`].
///
/// A component instance keeps one such table, shared by every
/// resource type and every other handle kind the instance uses; the
/// host keeps one table per resource type for the handles it owns
/// outright. The table allocates 32-bit indices, frees them on drop,
/// and surfaces structured failure when a stale handle is presented.
/// Index 0 is never handed out, and a freed index is reused
/// deterministically from a free list. Borrow bookkeeping lives in
/// the entries: an owning entry counts the borrows lifted out of it,
/// and a borrow entry names the call it is owed to. This type's own
/// [`remove`](Self::remove) is unconditional; it is
/// [`HandleTables::remove_own`](super::HandleTables::remove_own)
/// that reads the count and refuses while it is above zero.
///
/// [`Store`]: crate::Store
pub struct HandleTable {
    slots: Vec<Slot>,
    free_head: Option<u32>,
}

impl HandleTable {
    /// Construct an empty handle table.
    pub fn new() -> Self {
        Self {
            slots: vec![Slot::Reserved],
            free_head: None,
        }
    }

    /// Insert an entry, of any kind, and return its index.
    pub fn insert_entry(&mut self, entry: HandleKind) -> u32 {
        self.insert(entry)
    }

    fn insert(&mut self, entry: HandleKind) -> u32 {
        if let Some(idx) = self.free_head {
            let (next, generation) = match self.slots[idx as usize] {
                Slot::Free { next, generation } => (next, generation),
                Slot::Occupied { .. } | Slot::Reserved => {
                    unreachable!("free_head pointed at a slot that is not free")
                }
            };
            self.free_head = next;
            self.slots[idx as usize] = Slot::Occupied { entry, generation };
            idx
        } else {
            let idx = self.slots.len() as u32;
            self.slots.push(Slot::Occupied {
                entry,
                generation: 0,
            });
            idx
        }
    }

    /// The resource rep at a live index, for an owning or borrowed
    /// entry.
    pub fn get(&self, index: u32) -> Option<u32> {
        self.entry(index).and_then(HandleKind::rep)
    }

    /// The generation of the entry at a live index: how many entries
    /// its slot held before it.
    pub fn generation(&self, index: u32) -> Option<u32> {
        match self.slots.get(index as usize)? {
            Slot::Occupied { generation, .. } => Some(*generation),
            Slot::Free { .. } | Slot::Reserved => None,
        }
    }

    /// The entry at a live index.
    pub fn entry(&self, index: u32) -> Option<&HandleKind> {
        match self.slots.get(index as usize)? {
            Slot::Occupied { entry, .. } => Some(entry),
            Slot::Free { .. } | Slot::Reserved => None,
        }
    }

    /// The entry at a live index, mutably.
    pub fn entry_mut(&mut self, index: u32) -> Option<&mut HandleKind> {
        match self.slots.get_mut(index as usize)? {
            Slot::Occupied { entry, .. } => Some(entry),
            Slot::Free { .. } | Slot::Reserved => None,
        }
    }

    /// Free a live index and return its entry.
    pub fn remove(&mut self, index: u32) -> Option<HandleKind> {
        let slot = self.slots.get_mut(index as usize)?;
        let (entry, generation) = match slot {
            Slot::Occupied { entry, generation } => (*entry, *generation),
            Slot::Free { .. } | Slot::Reserved => return None,
        };
        *slot = Slot::Free {
            next: self.free_head,
            generation: generation.wrapping_add(1),
        };
        self.free_head = Some(index);
        Some(entry)
    }
}

impl Default for HandleTable {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::internal::ResourceTypeIdInternal;

    fn own(rep: u32) -> HandleKind {
        HandleKind::Own {
            type_id: crate::resource::ResourceTypeId::fresh(),
            guest_defined: false,
            rep,
            lend_count: 0,
        }
    }

    #[wcmp_macros::test]
    fn it_mints_distinct_indices_while_live() {
        let mut table = HandleTable::new();
        let a = table.insert_entry(own(10));
        let b = table.insert_entry(own(20));
        assert_ne!(a, b);
        assert_eq!(table.get(a), Some(10));
        assert_eq!(table.get(b), Some(20));
    }

    #[wcmp_macros::test]
    fn it_reuses_freed_indices_deterministically() {
        let mut table = HandleTable::new();
        let a = table.insert_entry(own(1));
        let _b = table.insert_entry(own(2));
        assert_eq!(table.remove(a).and_then(|e| e.rep()), Some(1));

        // The next insert reuses the freshly-freed slot.
        let c = table.insert_entry(own(3));
        assert_eq!(c, a);
        assert_eq!(table.get(c), Some(3));
    }

    #[wcmp_macros::test]
    fn it_changes_the_generation_of_a_reused_slot() {
        let mut table = HandleTable::new();
        let a = table.insert_entry(own(1));
        let first = table.generation(a).expect("a live entry");
        table.remove(a);
        assert_eq!(table.generation(a), None, "a free slot has no live entry");
        let b = table.insert_entry(own(1));
        assert_eq!(b, a, "the slot is reused");
        assert_ne!(
            table.generation(b),
            Some(first),
            "the entry that reused the slot is told from the one before it"
        );
    }

    #[wcmp_macros::test]
    fn it_rejects_stale_indices_after_remove() {
        let mut table = HandleTable::new();
        let idx = table.insert_entry(own(7));
        assert_eq!(table.remove(idx).and_then(|e| e.rep()), Some(7));
        assert_eq!(table.get(idx), None);
        assert_eq!(table.remove(idx), None);
    }

    #[wcmp_macros::test]
    fn it_never_hands_out_index_zero() {
        let mut table = HandleTable::new();
        assert_eq!(table.insert_entry(own(5)), 1, "the first allocation is 1");
        assert_eq!(table.get(0), None);
        assert_eq!(table.remove(0), None);
    }

    #[wcmp_macros::test]
    fn it_rejects_never_allocated_indices() {
        let table = HandleTable::new();
        assert_eq!(table.get(42), None);
    }
}
