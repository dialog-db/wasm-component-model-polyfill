//! A slab of records addressed by a 32-bit index.

/// A slab of records addressed by a 32-bit index.
///
/// The store keeps one of these for each record kind the
/// concurrency model defines — tasks, subtasks, and threads. A
/// record is inserted when the call or thread it describes begins
/// and removed when it ends; a freed index returns to a free list
/// and is reused before the slab grows, so a store that runs many
/// short calls does not grow a slot per call.
///
/// Because indices are reused, an index alone does not name one
/// record for longer than that record lives. Each slot therefore
/// also carries a generation, which the removal of a record
/// advances. An identity built from an index and the generation the
/// slot carried at the time names that one record and nothing that
/// takes the index after it.
///
/// Unlike the handle tables, index zero is a real index here: these
/// indices never reach a guest on their own. A guest that names a
/// record does so through a handle-table entry, which carries the
/// index inside it.
pub struct RecordTable<T> {
    slots: Vec<Slot<T>>,
    free: Vec<u32>,
}

/// One slot of the slab: the record that lives there, if any, and
/// the generation that tells apart the records that have held it.
struct Slot<T> {
    record: Option<T>,
    generation: u32,
}

impl<T> RecordTable<T> {
    /// Construct an empty slab.
    pub fn new() -> Self {
        Self {
            slots: Vec::new(),
            free: Vec::new(),
        }
    }

    /// The index the next [`insert`](Self::insert) will hand out. A
    /// record that must know its own identity as it is built is
    /// built against this index and inserted straight after.
    pub fn next_index(&self) -> u32 {
        match self.free.last() {
            Some(index) => *index,
            None => self.slots.len() as u32,
        }
    }

    /// The generation the slot at `index` carries now, which is the
    /// generation the next record to live there will be known by. A
    /// slot the slab has not grown to yet carries zero, the
    /// generation a fresh slot starts at.
    pub fn generation(&self, index: u32) -> u32 {
        match self.slots.get(index as usize) {
            Some(slot) => slot.generation,
            None => 0,
        }
    }

    /// Insert a record and return the index that names it.
    pub fn insert(&mut self, record: T) -> u32 {
        match self.free.pop() {
            Some(index) => {
                self.slots[index as usize].record = Some(record);
                index
            }
            None => {
                self.slots.push(Slot {
                    record: Some(record),
                    generation: 0,
                });
                (self.slots.len() - 1) as u32
            }
        }
    }

    /// Borrow the record at `index`, or `None` when nothing lives
    /// there.
    pub fn get(&self, index: u32) -> Option<&T> {
        self.slots
            .get(index as usize)
            .and_then(|slot| slot.record.as_ref())
    }

    /// Mutably borrow the record at `index`, or `None` when nothing
    /// lives there.
    pub fn get_mut(&mut self, index: u32) -> Option<&mut T> {
        self.slots
            .get_mut(index as usize)
            .and_then(|slot| slot.record.as_mut())
    }

    /// Remove the record at `index` and return it. The slot's
    /// generation advances, so every identity that named the removed
    /// record stops naming anything.
    pub fn remove(&mut self, index: u32) -> Option<T> {
        let slot = self.slots.get_mut(index as usize)?;
        let record = slot.record.take()?;
        slot.generation = slot.generation.wrapping_add(1);
        self.free.push(index);
        Some(record)
    }

    /// The number of records currently live in the slab.
    pub fn len(&self) -> usize {
        self.slots.len() - self.free.len()
    }
}

impl<T> Default for RecordTable<T> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn it_advances_the_generation_of_a_slot_it_hands_out_again() {
        let mut table = RecordTable::new();
        let first = table.insert("first");
        assert_eq!(table.generation(first), 0);

        assert_eq!(table.remove(first), Some("first"));
        let second = table.insert("second");
        assert_eq!(second, first, "the freed index is handed out again");
        assert_ne!(
            table.generation(second),
            0,
            "the record that took the index is a different generation"
        );
    }

    #[test]
    fn it_reports_the_generation_of_a_slot_before_it_is_filled() {
        let table: RecordTable<()> = RecordTable::new();
        assert_eq!(
            table.generation(table.next_index()),
            0,
            "a slot the slab has not grown to yet starts at zero"
        );
    }
}
