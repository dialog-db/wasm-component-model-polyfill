//! The table of one component instance's threads.

use super::thread_id::ThreadId;

/// The table of one component instance's threads: the index space
/// `thread.index` answers in and the thread built-ins take an index
/// from.
///
/// It is the reference's `ComponentInstance.threads`, a `Table` of
/// its own beside the instance's handle table. Index zero is never
/// handed out, so the first thread takes index one. A removed index
/// goes on a free list and the next insertion takes the index freed
/// last, as the reference's `Table.add` and Wasmtime's handle table
/// both do, so a guest sees the same indices under every engine.
pub struct ThreadTable {
    /// The thread at each index, `None` where the index is free.
    /// Slot zero is the reserved index and stays `None`.
    slots: Vec<Option<ThreadId>>,
    /// The freed indices, the one freed last at the end.
    free: Vec<u32>,
}

impl ThreadTable {
    /// The largest index the table hands out, the reference's
    /// `Table.MAX_LENGTH`.
    const MAX_INDEX: u32 = (1 << 28) - 1;

    /// Construct an empty table.
    pub fn new() -> Self {
        Self {
            slots: vec![None],
            free: Vec::new(),
        }
    }

    /// Put `thread` at the next free index and return that index.
    /// `None` when every index up to the reference's limit is taken.
    pub fn insert(&mut self, thread: ThreadId) -> Option<u32> {
        if let Some(index) = self.free.pop() {
            self.slots[index as usize] = Some(thread);
            return Some(index);
        }
        let index = u32::try_from(self.slots.len()).ok()?;
        if index > Self::MAX_INDEX {
            return None;
        }
        self.slots.push(Some(thread));
        Some(index)
    }

    /// The thread at `index`, or `None` when the index is free or out
    /// of range.
    pub fn get(&self, index: u32) -> Option<ThreadId> {
        self.slots.get(index as usize).copied().flatten()
    }

    /// Free `index` when it holds `thread`, and leave the table as it
    /// is otherwise. A thread whose index another thread has already
    /// taken is not in the table any more, so its removal must not
    /// free the other thread's index.
    pub fn remove(&mut self, index: u32, thread: ThreadId) {
        if self.get(index) == Some(thread) {
            self.slots[index as usize] = None;
            self.free.push(index);
        }
    }
}

impl Default for ThreadTable {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn thread(index: u32) -> ThreadId {
        ThreadId::new(index, 0)
    }

    #[wcmp_macros::test]
    fn it_hands_out_indices_from_one() {
        let mut table = ThreadTable::new();
        assert_eq!(table.insert(thread(10)), Some(1));
        assert_eq!(table.insert(thread(11)), Some(2));
        assert_eq!(table.get(0), None, "index zero is reserved");
        assert_eq!(table.get(1), Some(thread(10)));
        assert_eq!(table.get(2), Some(thread(11)));
        assert_eq!(table.get(3), None);
    }

    #[wcmp_macros::test]
    fn it_reuses_the_index_freed_last() {
        let mut table = ThreadTable::new();
        for index in 0..3 {
            table.insert(thread(index));
        }
        table.remove(1, thread(0));
        table.remove(3, thread(2));
        assert_eq!(table.get(1), None);
        assert_eq!(table.insert(thread(7)), Some(3));
        assert_eq!(table.insert(thread(8)), Some(1));
        assert_eq!(table.insert(thread(9)), Some(4));
    }

    #[wcmp_macros::test]
    fn it_leaves_an_index_another_thread_took() {
        let mut table = ThreadTable::new();
        table.insert(thread(0));
        table.remove(1, thread(0));
        table.insert(thread(1));
        table.remove(1, thread(0));
        assert_eq!(
            table.get(1),
            Some(thread(1)),
            "a stale removal frees nothing"
        );
    }
}
