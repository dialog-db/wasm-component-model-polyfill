//! Why a handle-table lookup found no usable entry.

use std::fmt;

/// The reason a handle index did not name the entry a caller
/// expected, or the entry it named could not be used the way the
/// caller asked. Three messages for a resource lookup match the
/// plain strings Wasmtime's handle table raises word for word, and
/// the tests pin them: an unknown index, an entry lent out, and a
/// guest-defined resource of the wrong type. A guest that misuses a
/// resource handle in one of those ways fails the same way on both.
/// `NotOwned`, and `WrongType` where one side is host-defined, are
/// worded differently from Wasmtime. `WrongKind` has no Wasmtime
/// counterpart: it fires only when a resource lookup lands on a
/// non-resource entry, which a well-formed adapter never generates.
/// The three waitable causes follow Wasmtime's own wording for the
/// same misuse, which it raises from its handle table rather than as
/// a named trap, with the handle index added: Wasmtime says only
/// that the handle is not a waitable, a waitable set, or a subtask,
/// and a guest that misuses one index among many is better served by
/// being told which.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HandleLookupError {
    /// No live entry sits at the index.
    Unknown { index: u32 },
    /// A live entry sits there, but it holds a resource of another
    /// type. Each flag says whether a component defines the type.
    WrongType {
        index: u32,
        expected_guest: bool,
        found_guest: bool,
    },
    /// A live entry sits there, but it is not a resource at all: a
    /// subtask, a waitable set, or one of the kinds a later feature
    /// reserves.
    WrongKind { index: u32 },
    /// A live entry sits there, but it is not a waitable: a
    /// `waitable.join` named it as the waitable to join.
    NotAWaitable { index: u32 },
    /// A live entry sits there, but it is not a waitable set: one of
    /// the waitable set built-ins named it as the set to work on.
    NotAWaitableSet { index: u32 },
    /// A live entry sits there, but it is not a subtask:
    /// `subtask.drop` named it as the subtask to drop.
    NotASubtask { index: u32 },
    /// The entry is an owning entry lent out as a borrow, so it
    /// cannot be removed until the call that lent it ends.
    Lent,
    /// The caller wanted an owning entry, but the index names a
    /// borrow.
    NotOwned { index: u32 },
    /// A borrow was lifted out of an owning entry with no call to
    /// lend the entry to: either no scope is in flight, or the scope
    /// the caller named has already ended. Nothing gives the lend
    /// back in either case, so the lend is refused instead of made.
    NoCallInFlight,
}

impl HandleLookupError {
    fn definer(guest: bool) -> &'static str {
        if guest {
            "guest-defined"
        } else {
            "host-defined"
        }
    }
}

impl fmt::Display for HandleLookupError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unknown { index } => write!(f, "unknown handle index {index}"),
            Self::WrongType {
                index,
                expected_guest,
                found_guest,
            } => write!(
                f,
                "handle index {index} used with the wrong type, expected {} resource but found a different {} resource",
                Self::definer(*expected_guest),
                Self::definer(*found_guest)
            ),
            Self::WrongKind { index } => {
                write!(f, "handle index {index} does not name a resource")
            }
            Self::NotAWaitable { index } => {
                write!(f, "handle index {index} is not a waitable")
            }
            Self::NotAWaitableSet { index } => {
                write!(f, "handle index {index} is not a waitable-set")
            }
            Self::NotASubtask { index } => {
                write!(f, "handle index {index} is not a subtask")
            }
            Self::Lent => write!(f, "cannot remove owned resource while borrowed"),
            Self::NotOwned { index } => {
                write!(f, "handle index {index} is a borrow, not an owned resource")
            }
            Self::NoCallInFlight => {
                write!(f, "a borrow can only be lifted out during a call")
            }
        }
    }
}

impl std::error::Error for HandleLookupError {}

#[cfg(test)]
mod tests {
    use super::*;

    // Wasmtime raises these three from its handle table as plain
    // strings, not as trap codes, so there is no `Trap` to render and
    // compare against. The expected text is Wasmtime's format string
    // with the index filled in: `unknown handle index {idx}` and
    // `cannot remove owned resource while borrowed` in
    // `src/runtime/vm/component/handle_table.rs`, and the
    // `ResourceTypeMismatch` rendering in
    // `src/runtime/vm/component/resources.rs`, of the `wasmtime`
    // release the workspace pins. The conformance corpus matches
    // them by substring, so a message that drifts from Wasmtime's
    // shows up here first.

    #[wcmp_macros::test]
    fn it_pins_the_unknown_index_message_to_wasmtimes_wording() {
        assert_eq!(
            HandleLookupError::Unknown { index: 3 }.to_string(),
            "unknown handle index 3"
        );
    }

    #[wcmp_macros::test]
    fn it_pins_the_wrong_type_message_to_wasmtimes_wording() {
        assert_eq!(
            HandleLookupError::WrongType {
                index: 1,
                expected_guest: true,
                found_guest: true,
            }
            .to_string(),
            "handle index 1 used with the wrong type, expected guest-defined resource \
             but found a different guest-defined resource"
        );
    }

    #[wcmp_macros::test]
    fn it_pins_the_lent_message_to_wasmtimes_wording() {
        assert_eq!(
            HandleLookupError::Lent.to_string(),
            "cannot remove owned resource while borrowed"
        );
    }
}
