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

    /// Decoding a component binary failed.
    ///
    /// Surfaced when the bytes handed to the parser do not form a
    /// valid component: a corrupted preamble, a truncated section,
    /// or an encoding the polyfill does not yet implement. The
    /// underlying parser cause is captured as the source.
    #[error("failed to parse the component binary")]
    Parse(#[source] anyhow::Error),
}

/// A `Result` whose error variant is the polyfill's [`Error`].
pub type Result<T> = core::result::Result<T, Error>;
