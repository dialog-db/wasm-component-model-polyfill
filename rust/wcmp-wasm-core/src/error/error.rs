//! The one error type of the runtime layer.

use thiserror::Error;

use crate::capability::Capability;
use crate::error::TrapKind;

/// Every error the runtime layer returns.
///
/// Every fallible method of the runtime layer, and of each backend, returns
/// one of these. None of them panics or aborts. The set grows additively,
/// so a host matches it with a wildcard arm.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum Error {
    /// The backend does not declare the capability that the operation
    /// needs.
    #[error("the backend does not support `{0}`")]
    Unsupported(Capability),

    /// The engine refused to compile a module. The message is the engine's.
    #[error("the engine refused the module: {message}")]
    Compile {
        /// The message of the engine.
        message: String,
    },

    /// An instantiation was given a different number of imports than the
    /// module declares.
    #[error("the module declares {expected} imports, and the instantiation gave {actual}")]
    ImportCount {
        /// The number of imports the module declares.
        expected: usize,
        /// The number of externs the instantiation gave.
        actual: usize,
    },

    /// An import was given an extern of the wrong kind or type.
    ///
    /// The engine refused the instantiation for its imports. Where the
    /// backend cannot tell which import the engine refused, both names are
    /// empty.
    #[error("the import `{module}` `{name}` does not link: {message}")]
    Link {
        /// The module name of the import.
        module: String,
        /// The item name of the import.
        name: String,
        /// Why the extern does not fit, in the words of the engine.
        message: String,
    },

    /// The guest trapped.
    #[error(transparent)]
    Trap(#[from] TrapKind),

    /// A memory access fell outside the memory.
    #[error("{len} bytes at offset {offset} fall outside the memory of {size} bytes")]
    MemoryOutOfBounds {
        /// The offset of the access.
        offset: u64,
        /// The length of the access, in bytes.
        len: u64,
        /// The size of the memory, in bytes.
        size: u64,
    },

    /// A table access fell outside the table.
    #[error("index {index} falls outside the table of {size} elements")]
    TableOutOfBounds {
        /// The index of the access.
        index: u64,
        /// The number of elements of the table.
        size: u64,
    },

    /// A memory or a table could not grow by the amount asked, because the
    /// growth would pass its maximum or the engine refused it.
    #[error("the memory or table could not grow by {delta}")]
    Grow {
        /// The growth asked for, in pages for a memory and in elements for
        /// a table.
        delta: u64,
    },

    /// A value, or a number of values, did not match the type it was given
    /// for: an argument of a call, a result slot, or the value of a global
    /// or a table element.
    #[error("type mismatch: {message}")]
    TypeMismatch {
        /// What did not match, in the words of the engine.
        message: String,
    },

    /// An object was used with a store that does not own it.
    #[error("the object belongs to another store")]
    WrongStore,

    /// A module was used with a store of another engine.
    #[error("the module belongs to another engine")]
    WrongEngine,

    /// The backend failed in a way no other case describes.
    #[error("the backend failed: {message}")]
    Backend {
        /// What failed, in the words of the backend.
        message: String,
    },
}

/// The result of every fallible method of the runtime layer.
pub type Result<T> = core::result::Result<T, Error>;
