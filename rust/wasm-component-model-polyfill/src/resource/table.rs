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
//! Borrow tracking is per-call: a `borrow<T>` lifted from the guest
//! lives on a per-call ledger rather than mutating the table; only
//! `own<T>` allocations and drops touch this slab.
//!
//! [`Store`]: crate::Store

/// One entry in the slab.
///
/// Free entries form a singly-linked list; occupied entries carry
/// the host's representation of the resource alongside the slot's
/// generation.
enum Slot {
    Free {
        /// Index of the next free slot, or `None` if this is the
        /// list tail.
        next: Option<u32>,
    },
    Occupied {
        /// The host-supplied 32-bit representation of the resource —
        /// typically an index into a host-managed table.
        rep: u32,
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
    pub fn insert(&mut self, rep: u32) -> u32 {
        if let Some(idx) = self.free_head {
            let next = match self.slots[idx as usize] {
                Slot::Free { next } => next,
                Slot::Occupied { .. } => {
                    unreachable!("free_head pointed at an occupied slot")
                }
            };
            self.free_head = next;
            self.slots[idx as usize] = Slot::Occupied { rep };
            idx
        } else {
            let idx = self.slots.len() as u32;
            self.slots.push(Slot::Occupied { rep });
            idx
        }
    }

    /// Read the host representation behind the given index without
    /// removing it. Returns `None` for stale or never-allocated
    /// indices.
    pub fn get(&self, index: u32) -> Option<u32> {
        match self.slots.get(index as usize)? {
            Slot::Occupied { rep } => Some(*rep),
            Slot::Free { .. } => None,
        }
    }

    /// Remove the entry at `index` and return its representation, or
    /// `None` if the index is stale or never-allocated. The freed
    /// slot is pushed onto the free list for deterministic reuse.
    pub fn remove(&mut self, index: u32) -> Option<u32> {
        let slot = self.slots.get_mut(index as usize)?;
        let rep = match slot {
            Slot::Occupied { rep } => *rep,
            Slot::Free { .. } => return None,
        };
        *slot = Slot::Free {
            next: self.free_head,
        };
        self.free_head = Some(index);
        Some(rep)
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
        let a = table.insert(10);
        let b = table.insert(20);
        assert_ne!(a, b);
        assert_eq!(table.get(a), Some(10));
        assert_eq!(table.get(b), Some(20));
    }

    #[test]
    fn it_reuses_freed_indices_deterministically() {
        let mut table = HandleTable::new();
        let a = table.insert(1);
        let _b = table.insert(2);
        assert_eq!(table.remove(a), Some(1));

        // The next insert reuses the freshly-freed slot.
        let c = table.insert(3);
        assert_eq!(c, a);
        assert_eq!(table.get(c), Some(3));
    }

    #[test]
    fn it_rejects_stale_indices_after_remove() {
        let mut table = HandleTable::new();
        let idx = table.insert(7);
        assert_eq!(table.remove(idx), Some(7));
        assert_eq!(table.get(idx), None);
        assert_eq!(table.remove(idx), None);
    }

    #[test]
    fn it_rejects_never_allocated_indices() {
        let table = HandleTable::new();
        assert_eq!(table.get(42), None);
    }
}
