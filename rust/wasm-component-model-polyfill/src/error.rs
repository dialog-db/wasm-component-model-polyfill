//! The polyfill's single error type.
//!
//! Every public function that can fail returns [`Result<T, Error>`]
//! (aliased to [`Result<T>`] at the crate root). The variant set
//! grows additively as the polyfill grows; the polyfill does not
//! introduce parallel error hierarchies for each subsystem.
//!
//! Underlying causes are captured as `#[source]` fields so the
//! origin of an error is preserved without leaking the runtime
//! layer's types into the public API. [`anyhow::Error`] appears in
//! `#[source]` fields only because the runtime layer surfaces
//! fallible operations as `anyhow::Result`; consumers should treat
//! the inner cause as opaque.

use thiserror::Error;

/// Every error the polyfill can return.
///
/// The variant set grows additively as later slices land; consumers
/// should match it as `#[non_exhaustive]` (the attribute is enforced
/// at the type level below).
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum Error {
    /// The runtime-layer backend engine could not be constructed.
    #[error("failed to construct the backend engine")]
    BackendEngineConstruction(#[source] anyhow::Error),

    /// The runtime-layer backend store could not be constructed.
    #[error("failed to construct the backend store")]
    BackendStoreCreation(#[source] anyhow::Error),

    /// The bytes did not start with the component preamble, or
    /// failed binary-format validation while being decoded.
    /// Carries the structured reason and the byte offset at which
    /// the parser tripped.
    #[error("invalid component binary at offset {offset}: {message}")]
    InvalidComponentBinary {
        /// A short human-readable description of what failed.
        message: String,
        /// The byte offset at which the parser detected the
        /// failure.
        offset: usize,
    },

    /// The bytes were a core WebAssembly module rather than a
    /// component.
    #[error("expected a component binary, got a core module")]
    NotAComponent,

    /// A type-index reference fell outside the component's type
    /// space. The component declared at most `_index` types when
    /// the lookup happened.
    #[error("type index {index} is out of bounds for the component's type space")]
    TypeIndexOutOfBounds {
        /// The out-of-bounds index.
        index: u32,
    },

    /// A type-space slot was the wrong kind for the place it was
    /// referenced from. For example, an export that names a
    /// function used a type index whose slot held a record.
    #[error("expected type index {index} to refer to {expected}, found {actual}")]
    WrongTypeKind {
        /// The type index whose kind disagreed with the use site.
        index: u32,
        /// What the use site expected.
        expected: &'static str,
        /// What the slot actually held.
        actual: &'static str,
    },
}

/// A `Result` whose error variant is the polyfill's [`Error`].
pub type Result<T> = core::result::Result<T, Error>;
