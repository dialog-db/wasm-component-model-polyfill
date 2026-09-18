//! The mark a running turn leaves, and its pairing with the end of
//! the turn.

use core::task::Waker;
use std::sync::{Arc, Mutex, MutexGuard};

use crate::resource::HandleTables;

/// The mark a running turn leaves, and its pairing with the end of
/// the turn.
///
/// A turn marks itself as running, so that a driver entered from
/// inside it fails with the recursive-driver cause, and it records
/// the waker a trampoline polls a host task with. Both belong to the
/// turn and must go back when the turn ends.
///
/// The turn is not in a position to put them back itself. Its body
/// runs items, an item runs guest work, and an accessor runs a
/// closure the embedder wrote; any of those can panic. A panic that
/// left the mark behind would leave the store refusing every later
/// driver for the rest of its life. The mark is therefore held by
/// this guard, and given back when the guard is dropped — whether
/// the turn returned or unwound.
///
/// A panic taken while the tables were locked poisons the lock as
/// well, and a poisoned lock refuses every later reader. The guard
/// takes the tables back from the poison and clears it, on the way
/// in and on the way out. It is the one place that does. Poisoning
/// says that some value behind the lock may be half-written, and
/// nothing behind this lock is: a store is `Send` and not `Sync`, so
/// the lock guards no concurrent writer, and what a panic can
/// interrupt is one record's worth of bookkeeping the store is free
/// to read afterwards. Leaving the poison would turn every panic a
/// turn survived into a store that fails every later call, which is
/// exactly the outcome this guard exists to prevent.
pub struct TurnGuard {
    tables: Arc<Mutex<HandleTables>>,
    displaced: Option<Waker>,
}

impl TurnGuard {
    /// Mark a turn of the store that owns `tables` as running, with
    /// `waker` as the waker a trampoline polls a host task with. The
    /// waker this turn displaced comes back when the guard is
    /// dropped, which is how a turn nested in another leaves the
    /// outer one as it found it.
    pub fn enter(tables: &Arc<Mutex<HandleTables>>, waker: &Waker) -> Self {
        let displaced = Self::take(tables).scheduler.enter_turn(waker);
        Self {
            tables: tables.clone(),
            displaced,
        }
    }

    /// Whether a turn of the store that owns `tables` is running.
    ///
    /// This is the question both driver entries ask before they
    /// build a guard of their own, and it is asked here so that it
    /// is asked through the same poison recovery the guard itself
    /// uses. A panic that poisoned the tables while no turn was
    /// running would otherwise refuse every later driver — the
    /// outcome this type exists to prevent, arrived at one step
    /// earlier.
    pub fn in_turn(tables: &Arc<Mutex<HandleTables>>) -> bool {
        Self::take(tables).scheduler.in_turn()
    }

    /// Lock `tables`, taking them back from a poison a panic left
    /// and clearing it, for the reason the type's documentation
    /// gives.
    fn take(tables: &Arc<Mutex<HandleTables>>) -> MutexGuard<'_, HandleTables> {
        match tables.lock() {
            Ok(guard) => guard,
            Err(poisoned) => {
                tables.clear_poison();
                poisoned.into_inner()
            }
        }
    }
}

impl Drop for TurnGuard {
    fn drop(&mut self) {
        Self::take(&self.tables)
            .scheduler
            .leave_turn(self.displaced.take());
    }
}
