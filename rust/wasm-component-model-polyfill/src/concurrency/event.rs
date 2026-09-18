//! One event, ready for the thread that takes delivery of it.

use super::event_code::EventCode;
use super::subtask_state::SubtaskState;

/// One event: a code and two payloads, whose meaning the code
/// settles.
///
/// A waitable holds at most one event at a time, in its pending event
/// slot. The scheduler records readiness by filling the slot, and a
/// thread that waits on or polls a set containing the waitable takes
/// the event out of it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Event {
    /// What happened.
    code: EventCode,
    /// The two payloads, whose meaning follows the code.
    payloads: [u32; 2],
}

impl Event {
    /// The event that says nothing was ready: code
    /// [`EventCode::None`] and two zero payloads. A poll of a set
    /// that holds no event answers with this.
    pub fn none() -> Self {
        Self {
            code: EventCode::None,
            payloads: [0; 2],
        }
    }

    /// The event a subtask delivers: its index in the caller
    /// instance's handle table and the state it moved to.
    pub fn subtask(handle_index: u32, state: SubtaskState) -> Self {
        Self {
            code: EventCode::Subtask,
            payloads: [handle_index, state.value()],
        }
    }

    /// The event a finished copy delivers: the stream or future
    /// end's index in the handle table and the copy result. `code`
    /// is the read or write code of the end's kind. Nothing
    /// constructs one yet; the features that add streams and futures
    /// do.
    #[allow(dead_code)]
    pub fn copy(code: EventCode, handle_index: u32, result: u32) -> Self {
        Self {
            code,
            payloads: [handle_index, result],
        }
    }

    /// The event a cancelled task's waiting thread receives. Both
    /// payloads are zero. Nothing constructs one yet; the feature
    /// that adds cancellation does.
    #[allow(dead_code)]
    pub fn task_cancelled() -> Self {
        Self {
            code: EventCode::TaskCancelled,
            payloads: [0; 2],
        }
    }

    /// What happened.
    pub fn code(self) -> EventCode {
        self.code
    }

    /// The two payloads, whose meaning follows the code.
    pub fn payloads(self) -> [u32; 2] {
        self.payloads
    }

    /// The event as the three numbers a guest sees: the code and the
    /// two payloads. A built-in that writes an event into guest
    /// memory writes exactly these.
    pub fn triple(self) -> (u32, u32, u32) {
        (self.code.value(), self.payloads[0], self.payloads[1])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[wcmp_macros::test]
    fn it_numbers_every_event_code_as_the_reference_does() {
        assert_eq!(Event::none().triple(), (0, 0, 0));
        assert_eq!(
            Event::subtask(5, SubtaskState::Started).triple(),
            (1, 5, 1),
            "a subtask event carries the handle index and the state"
        );
        assert_eq!(Event::copy(EventCode::StreamRead, 2, 7).triple(), (2, 2, 7));
        assert_eq!(
            Event::copy(EventCode::StreamWrite, 2, 7).triple(),
            (3, 2, 7)
        );
        assert_eq!(Event::copy(EventCode::FutureRead, 2, 7).triple(), (4, 2, 7));
        assert_eq!(
            Event::copy(EventCode::FutureWrite, 2, 7).triple(),
            (5, 2, 7)
        );
        assert_eq!(Event::task_cancelled().triple(), (6, 0, 0));
    }

    #[wcmp_macros::test]
    fn it_carries_each_resolved_subtask_state_as_its_second_payload() {
        for (state, value) in [
            (SubtaskState::Returned, 2),
            (SubtaskState::CancelledBeforeStarted, 3),
            (SubtaskState::CancelledBeforeReturned, 4),
        ] {
            let event = Event::subtask(1, state);
            assert_eq!(event.code(), EventCode::Subtask);
            assert_eq!(event.payloads(), [1, value]);
        }
    }
}
