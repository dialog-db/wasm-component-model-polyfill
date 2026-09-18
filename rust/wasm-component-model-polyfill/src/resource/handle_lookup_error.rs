//! Why a handle-table lookup found no usable entry.

use std::fmt;

/// The reason a handle index did not name the entry a caller
/// expected, or the entry it named could not be used the way the
/// caller asked. The messages for a resource lookup match Wasmtime's
/// traps word for word, so a guest that misuses a resource handle
/// fails the same way on both. `WrongKind` has no Wasmtime
/// counterpart: it fires only when a resource lookup lands on a
/// non-resource entry, which a well-formed adapter never generates.
/// The two waitable causes read as Wasmtime's own messages for the
/// same misuse, which it raises from its handle table rather than as
/// a named trap.
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
