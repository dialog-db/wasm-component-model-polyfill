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
/// Unlike the handle tables, index zero is a real index here: these
/// indices never reach a guest on their own. A guest that names a
/// record does so through a handle-table entry, which carries the
/// index inside it.
pub struct RecordTable<T> {
    slots: Vec<Option<T>>,
    free: Vec<u32>,
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

    /// Insert a record and return the index that names it.
    pub fn insert(&mut self, record: T) -> u32 {
        match self.free.pop() {
            Some(index) => {
                self.slots[index as usize] = Some(record);
                index
            }
            None => {
                self.slots.push(Some(record));
                (self.slots.len() - 1) as u32
            }
        }
    }

    /// Borrow the record at `index`, or `None` when nothing lives
    /// there.
    pub fn get(&self, index: u32) -> Option<&T> {
        self.slots.get(index as usize).and_then(Option::as_ref)
    }

    /// Mutably borrow the record at `index`, or `None` when nothing
    /// lives there.
    pub fn get_mut(&mut self, index: u32) -> Option<&mut T> {
        self.slots.get_mut(index as usize).and_then(Option::as_mut)
    }

    /// Remove the record at `index` and return it.
    pub fn remove(&mut self, index: u32) -> Option<T> {
        let record = self.slots.get_mut(index as usize).and_then(Option::take)?;
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
