//! Why a handle-table lookup found no usable entry.

use std::fmt;

/// The reason a handle index did not name the entry a caller
/// expected. The messages match Wasmtime's traps word for word, so a
/// guest that misuses a handle fails the same way on both.
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
    /// The entry is an owning entry lent out as a borrow, so it
    /// cannot be removed until the call that lent it ends.
    Lent,
    /// The caller wanted an owning entry, but the index names a
    /// borrow.
    NotOwned { index: u32 },
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
            Self::Lent => write!(f, "cannot remove owned resource while borrowed"),
            Self::NotOwned { index } => {
                write!(f, "handle index {index} is a borrow, not an owned resource")
            }
        }
    }
}

impl std::error::Error for HandleLookupError {}
