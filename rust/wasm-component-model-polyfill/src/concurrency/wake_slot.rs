//! Where one side leaves a value for a side that waits, with the
//! waker that brings the waiting side back.

use core::fmt;
use core::task::Waker;
use std::sync::{Arc, Mutex};

/// A slot whose filling wakes whoever waits on it.
///
/// A caller that is not on the stack when its call resolves watches
/// a slot of this shape: what resolves the call fills the slot from
/// inside a turn, with the caller's future nowhere on the stack, and
/// the future takes the value out when it is next polled.
///
/// The waker lives beside the value because that next poll is not
/// something the slot may assume. A driver consults its condition
/// after every turn it runs, so a future a driver owns is polled
/// again on its own. A future a host combinator owns is not: anything
/// of the `FuturesUnordered` shape polls a child again only after
/// that child's waker fires. A filling that did not wake would leave
/// such a caller pending against a value already in the slot.
///
/// Both sides hold the slot, because it outlives the future that
/// waits on it: dropping that future cancels nothing.
pub struct WakeSlot<T> {
    slot: Arc<Mutex<Held<T>>>,
}

/// What one slot holds: the value, once a side has left one, and the
/// waker of the side that waits, while one is waiting.
struct Held<T> {
    value: Option<T>,
    waker: Option<Waker>,
}

impl<T> WakeSlot<T> {
    /// An empty slot, with nobody waiting on it yet.
    pub fn new() -> Self {
        Self {
            slot: Arc::new(Mutex::new(Held {
                value: None,
                waker: None,
            })),
        }
    }

    /// Leave `value` for the side that waits, and wake it. A slot
    /// that already held a value keeps the later one.
    ///
    /// The wake happens after the slot's lock is released, so that
    /// whatever the waker runs does not meet the slot locked.
    pub fn fill(&self, value: T) {
        let woken = match self.slot.lock() {
            Ok(mut held) => {
                held.value = Some(value);
                held.waker.take()
            }
            Err(_) => None,
        };
        if let Some(waker) = woken {
            waker.wake();
        }
    }

    /// Take the value out, leaving the slot empty, when a side has
    /// left one. A value is delivered once.
    pub fn take(&self) -> Option<T> {
        self.slot.lock().ok().and_then(|mut held| held.value.take())
    }

    /// Take the value out as [`Self::take`] does, and remember
    /// `waker` when there is no value yet, so that whatever fills the
    /// slot brings this side back.
    ///
    /// Both happen under the one lock. A value left between a look
    /// that found none and a registration made afterwards would be a
    /// wake nobody receives, and the side that waits would never be
    /// polled again.
    pub fn take_or_wait(&self, waker: &Waker) -> Option<T> {
        let Ok(mut held) = self.slot.lock() else {
            return None;
        };
        if let Some(value) = held.value.take() {
            return Some(value);
        }
        if held
            .waker
            .as_ref()
            .is_none_or(|kept| !kept.will_wake(waker))
        {
            held.waker = Some(waker.clone());
        }
        None
    }
}

impl<T> Clone for WakeSlot<T> {
    /// The other half of the same slot. The value is not cloned: the
    /// two halves are one slot, which is what makes one side's fill
    /// the other side's take.
    fn clone(&self) -> Self {
        Self {
            slot: self.slot.clone(),
        }
    }
}

impl<T> Default for WakeSlot<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T: fmt::Debug> fmt::Debug for WakeSlot<T> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.slot.lock() {
            Ok(held) => formatter
                .debug_struct("WakeSlot")
                .field("value", &held.value)
                .field("waiting", &held.waker.is_some())
                .finish(),
            Err(_) => formatter.write_str("WakeSlot(poisoned)"),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    /// A waker that counts the wakes it received, which is what the
    /// side that fills a slot is measured by.
    #[derive(Default)]
    struct Wakes(AtomicUsize);

    impl Wakes {
        fn count(&self) -> usize {
            self.0.load(Ordering::Relaxed)
        }
    }

    impl std::task::Wake for Wakes {
        fn wake(self: Arc<Self>) {
            self.0.fetch_add(1, Ordering::Relaxed);
        }

        fn wake_by_ref(self: &Arc<Self>) {
            self.0.fetch_add(1, Ordering::Relaxed);
        }
    }

    #[wcmp_macros::test]
    fn it_hands_the_value_it_was_filled_with_to_the_other_half() {
        let filled: WakeSlot<u32> = WakeSlot::new();
        let waiting = filled.clone();

        filled.fill(7);

        assert_eq!(waiting.take(), Some(7));
        assert_eq!(waiting.take(), None, "a value is delivered once");
    }

    #[wcmp_macros::test]
    fn it_wakes_the_side_that_waits_when_it_is_filled() {
        let wakes = Arc::new(Wakes::default());
        let waker = Waker::from(wakes.clone());
        let slot: WakeSlot<u32> = WakeSlot::new();

        assert_eq!(slot.take_or_wait(&waker), None, "the slot is empty");
        assert_eq!(wakes.count(), 0, "nothing has filled it yet");

        slot.clone().fill(3);

        assert_eq!(
            wakes.count(),
            1,
            "the fill woke the side that was waiting on the empty slot"
        );
        assert_eq!(slot.take_or_wait(&waker), Some(3));
    }

    #[wcmp_macros::test]
    fn it_leaves_no_waker_behind_when_the_value_was_already_there() {
        let wakes = Arc::new(Wakes::default());
        let waker = Waker::from(wakes.clone());
        let slot: WakeSlot<u32> = WakeSlot::new();
        slot.fill(11);

        assert_eq!(slot.take_or_wait(&waker), Some(11));
        slot.fill(12);

        assert_eq!(
            wakes.count(),
            0,
            "the side that waits took the value instead of waiting, so the \
             next fill has nobody to wake"
        );
    }
}
