// Copyright 2026 The Dialog DB Project
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Where the event a queued callback item receives is left for it.

use std::sync::{Arc, Mutex};

use super::event::Event;
use super::waitable_set_id::WaitableSetId;

/// What a slot holds for the item: an event, or the waitable set the
/// item takes its event from when it runs.
enum Content {
    /// The event itself.
    Event(Event),
    /// The set whose next event the item takes as it runs.
    Set(WaitableSetId),
}

/// The one event a queued callback item receives when it runs.
///
/// A callback item is built before its event is known. A task that
/// returned the wait word on a set that holds nothing is queued only
/// when a later turn fills an event in one of the set's waitables, and
/// the turn that queues it is not the turn that built the item. The
/// slot is what the two sides share: the scheduler fills it as it
/// queues the item, and the item takes the event out as it runs.
///
/// A yield fills the slot at once, with the none event, because the
/// event a yield delivers is known before the item is queued. A wait
/// fills it with the set instead, and the item takes the set's event
/// only as it runs. That is where the reference and Wasmtime take it,
/// so a cancellation request that arrives while the item is queued
/// is delivered first, and the set keeps its event for the next wait.
///
/// An item that finds the slot empty receives the none event. That is
/// the event a thread resumed for a reason other than its own
/// readiness sees, and it is what the reference delivers when a wait
/// ends with nothing in the set.
#[derive(Clone)]
pub struct EventSlot {
    slot: Arc<Mutex<Option<Content>>>,
}

impl EventSlot {
    /// An empty slot, for an item whose event a later turn decides.
    pub fn new() -> Self {
        Self {
            slot: Arc::new(Mutex::new(None)),
        }
    }

    /// A slot holding `event`, for an item whose event is known as it
    /// is queued.
    pub fn holding(event: Event) -> Self {
        let slot = Self::new();
        slot.fill(event);
        slot
    }

    /// Leave `event` for the item that holds the other half of this
    /// slot. A slot that already held something keeps the later one,
    /// which is the rule a waitable's own pending event slot follows.
    pub fn fill(&self, event: Event) {
        self.put(Content::Event(event));
    }

    /// Leave `set` for the item that holds the other half of this
    /// slot: the item takes the set's next event when it runs.
    pub fn fill_from(&self, set: WaitableSetId) {
        self.put(Content::Set(set));
    }

    /// Take the set the slot was filled from, leaving the slot empty.
    /// `None`, and the slot untouched, when it holds an event or
    /// nothing.
    pub fn take_set(&self) -> Option<WaitableSetId> {
        let mut slot = self.slot.lock().ok()?;
        match *slot {
            Some(Content::Set(set)) => {
                *slot = None;
                Some(set)
            }
            _ => None,
        }
    }

    /// Take the event out, leaving the slot empty. A slot that holds
    /// nothing, or a set, answers with the none event.
    pub fn take(&self) -> Event {
        match self.slot.lock().ok().and_then(|mut slot| slot.take()) {
            Some(Content::Event(event)) => event,
            _ => Event::none(),
        }
    }

    /// Replace what the slot holds with `content`.
    fn put(&self, content: Content) {
        if let Ok(mut slot) = self.slot.lock() {
            *slot = Some(content);
        }
    }
}

impl Default for EventSlot {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::super::event_code::EventCode;
    use super::super::subtask_state::SubtaskState;
    use super::*;

    #[wcmp_macros::test]
    fn it_hands_the_event_it_was_filled_with_to_the_other_half() {
        let queued = EventSlot::new();
        let running = queued.clone();
        queued.fill(Event::subtask(3, SubtaskState::Returned));

        let event = running.take();

        assert_eq!(event.code(), EventCode::Subtask);
        assert_eq!(event.payloads(), [3, 2]);
    }

    #[wcmp_macros::test]
    fn it_answers_with_the_none_event_when_it_was_never_filled() {
        let slot = EventSlot::new();

        assert_eq!(slot.take().triple(), (0, 0, 0));
    }

    #[wcmp_macros::test]
    fn it_empties_itself_when_the_event_is_taken() {
        let slot = EventSlot::holding(Event::subtask(1, SubtaskState::Returned));

        assert_eq!(slot.take().code(), EventCode::Subtask);
        assert_eq!(
            slot.take().triple(),
            (0, 0, 0),
            "the event is delivered once"
        );
    }
}
