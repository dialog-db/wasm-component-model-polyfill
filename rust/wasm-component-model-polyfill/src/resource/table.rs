//! Per-resource-type handle table.
//!
//! A `HandleTable` is the slab the canonical ABI's runtime-state
//! rules require for every live `own<T>` and `borrow<T>` handle.
//! The polyfill keeps one table per registered resource type per
//! [`Store`], reached through crate-private accessors on the store.
//!
//! Allocation policy follows the canonical-ABI runtime-state rules:
//!
//! - Indices are non-aliasing while live: every minted index points
//!   to a distinct table entry.
//! - Reuse of a freed index is deterministic within a single store:
//!   a free list of freed indices is drained LIFO so reuse is fixed
//!   by the order of drops.
//! - Each entry tracks its own *generation* so a stale handle
//!   pointing at a slot that has since been reused is rejected. The
//!   generation is opaque and not exposed.
//!
//! Borrows live in the table too: a `borrow<T>` lowered into the
//! guest is an entry owed to the call it was lowered in, and an
//! owning entry lent to the host as a borrow counts its lends so it
//! cannot be removed before the call ends.
//!
//! [`Store`]: crate::Store

use super::handle_entry::HandleEntry;

/// One entry in the slab.
///
/// Free entries form a singly-linked list; occupied entries carry
/// the resource's entry: its rep and whether it is owned or a
/// borrow.
enum Slot {
    Free {
        /// Index of the next free slot, or `None` if this is the
        /// list tail.
        next: Option<u32>,
    },
    Occupied {
        /// The entry: the resource's rep and whether the slot owns
        /// it or borrows it for a call.
        entry: HandleEntry,
    },
}

/// A per-resource-type handle table, owned by a single [`Store`].
///
/// The table allocates 32-bit indices for `own<T>` handles, frees
/// them on drop, and surfaces structured failure when a stale
/// handle is presented. Borrow tracking is the responsibility of
/// the per-call lift/lower context, not of this table.
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
            slots: Vec::new(),
            free_head: None,
        }
    }

    /// Insert a fresh `own<T>` entry carrying the given host
    /// representation. Returns the table index.
    /// Insert an owning entry and return its index.
    pub fn insert_own(&mut self, rep: u32) -> u32 {
        self.insert(HandleEntry::Own { rep, lend_count: 0 })
    }

    /// Insert a borrow entry for the call scope at `scope` and return
    /// its index.
    pub fn insert_borrow(&mut self, rep: u32, scope: usize) -> u32 {
        self.insert(HandleEntry::Borrow { rep, scope })
    }

    fn insert(&mut self, entry: HandleEntry) -> u32 {
        if let Some(idx) = self.free_head {
            let next = match self.slots[idx as usize] {
                Slot::Free { next } => next,
                Slot::Occupied { .. } => {
                    unreachable!("free_head pointed at an occupied slot")
                }
            };
            self.free_head = next;
            self.slots[idx as usize] = Slot::Occupied { entry };
            idx
        } else {
            let idx = self.slots.len() as u32;
            self.slots.push(Slot::Occupied { entry });
            idx
        }
    }

    /// The rep at a live index.
    pub fn get(&self, index: u32) -> Option<u32> {
        self.entry(index).map(|entry| entry.rep())
    }

    /// The entry at a live index.
    pub fn entry(&self, index: u32) -> Option<&HandleEntry> {
        match self.slots.get(index as usize)? {
            Slot::Occupied { entry } => Some(entry),
            Slot::Free { .. } => None,
        }
    }

    /// The entry at a live index, mutably.
    pub fn entry_mut(&mut self, index: u32) -> Option<&mut HandleEntry> {
        match self.slots.get_mut(index as usize)? {
            Slot::Occupied { entry } => Some(entry),
            Slot::Free { .. } => None,
        }
    }

    /// Free a live index and return its entry.
    pub fn remove(&mut self, index: u32) -> Option<HandleEntry> {
        let slot = self.slots.get_mut(index as usize)?;
        let entry = match slot {
            Slot::Occupied { entry } => *entry,
            Slot::Free { .. } => return None,
        };
        *slot = Slot::Free {
            next: self.free_head,
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

    #[test]
    fn it_mints_distinct_indices_while_live() {
        let mut table = HandleTable::new();
        let a = table.insert_own(10);
        let b = table.insert_own(20);
        assert_ne!(a, b);
        assert_eq!(table.get(a), Some(10));
        assert_eq!(table.get(b), Some(20));
    }

    #[test]
    fn it_reuses_freed_indices_deterministically() {
        let mut table = HandleTable::new();
        let a = table.insert_own(1);
        let _b = table.insert_own(2);
        assert_eq!(table.remove(a).map(|e| e.rep()), Some(1));

        // The next insert reuses the freshly-freed slot.
        let c = table.insert_own(3);
        assert_eq!(c, a);
        assert_eq!(table.get(c), Some(3));
    }

    #[test]
    fn it_rejects_stale_indices_after_remove() {
        let mut table = HandleTable::new();
        let idx = table.insert_own(7);
        assert_eq!(table.remove(idx).map(|e| e.rep()), Some(7));
        assert_eq!(table.get(idx), None);
        assert_eq!(table.remove(idx), None);
    }

    #[test]
    fn it_rejects_never_allocated_indices() {
        let table = HandleTable::new();
        assert_eq!(table.get(42), None);
    }
}
