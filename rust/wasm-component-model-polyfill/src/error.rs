//! The polyfill's single error type.
//!
//! Per [PDD005], every public function that can fail returns
//! [`Result<T, Error>`][Result] (aliased to [`Result<T>`][Result] at the
//! crate root). Subsequent slices add variants to this same enum rather
//! than introducing parallel error hierarchies. Underlying causes from
//! [`wasm_runtime_layer`] are captured as `#[source]` fields so the
//! origin of an error is preserved without leaking the runtime layer's
//! types into the public API.
//!
//! Note on [`anyhow::Error`] in `#[source]` fields: it appears here
//! only because [`wasm_runtime_layer`] surfaces fallible operations as
//! `anyhow::Result`, and the polyfill must propagate whatever it
//! receives. Exposing `anyhow::Error` is a pragmatic compromise, not a
//! design preference — a future slice may replace these `#[source]`
//! captures with structured causes once the runtime-layer error story
//! firms up. Consumers should treat the inner cause as opaque.
//!
//! [PDD005]: ../../../../design/PDD005%20Library%20Foundations.md

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
}

/// A `Result` whose error variant is the polyfill's [`Error`].
pub type Result<T> = core::result::Result<T, Error>;
