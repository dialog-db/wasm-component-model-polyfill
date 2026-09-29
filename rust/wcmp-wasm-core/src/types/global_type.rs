//! The type of a global.

use crate::types::{Mutability, ValType};

/// The type of a global: the type of its value, and whether it can change.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct GlobalType {
    content: ValType,
    mutability: Mutability,
}

impl GlobalType {
    /// The type of a global that holds a `content` value.
    pub const fn new(content: ValType, mutability: Mutability) -> Self {
        Self {
            content,
            mutability,
        }
    }

    /// The type of the value the global holds.
    pub const fn content(&self) -> &ValType {
        &self.content
    }

    /// Whether the global can be set.
    pub const fn mutability(&self) -> Mutability {
        self.mutability
    }
}
