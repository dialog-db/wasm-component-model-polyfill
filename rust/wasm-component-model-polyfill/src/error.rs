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

use semver::Version;
use thiserror::Error;

use crate::component::ExternalName;
use crate::identifier::InterfaceIdentifier;

/// Every error the polyfill can return.
///
/// The variant set grows additively as later work lands; consumers
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

    /// A component import could not be resolved against the
    /// linker's registered linker instances.
    ///
    /// Resolution failures fall into a small set of structured
    /// shapes: the import has no candidate, more than one candidate
    /// is equally good, the candidate's version is outside the
    /// import's WIT-spec compatibility range, or the import requires
    /// a host item the polyfill does not yet support
    /// (every host-function and host-resource registration mode).
    #[error("link error: {0}")]
    Link(#[source] LinkError),

    /// The runtime substrate failed to instantiate a successfully
    /// linked component, or the polyfill rejected the component for
    /// a structural reason it intentionally defers
    /// (e.g. an exported signature that requires compound-valtype
    /// lift/lower).
    #[error("instantiation error: {0}")]
    Instantiation(#[source] InstantiationError),

    /// A polyfill-internal invariant that "shouldn't happen given
    /// upstream guarantees" was nevertheless violated. This
    /// variant exists so the polyfill never panics on inputs the
    /// user can produce; reaching it is a polyfill bug worth
    /// reporting.
    #[error("polyfill internal invariant violated: {message}")]
    Internal {
        /// Short description of which invariant was violated and
        /// where.
        message: String,
    },
}

/// The reason an import could not be resolved.
///
/// Surfaced as the inner cause of [`Error::Link`]. Each variant
/// carries enough context to identify the unresolved import in a
/// diagnostic without forcing the consumer to keep the full
/// candidate list around.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum LinkError {
    /// No registered linker instance matched the import's
    /// identifier or its WIT-spec compatibility range.
    #[error("no registered linker instance satisfies import `{import}`")]
    UnresolvedImport {
        /// The name of the import the linker could not satisfy.
        import: ExternalName,
    },

    /// More than one registered linker instance was equally good a
    /// match for the import, and the WIT-spec tie-break did not
    /// disambiguate.
    #[error("more than one registered linker instance satisfies import `{import}`: {candidates:?}")]
    AmbiguousImport {
        /// The name of the import that could not be uniquely
        /// resolved.
        import: ExternalName,
        /// The interfaces that all matched the import equally.
        candidates: Vec<InterfaceIdentifier>,
    },

    /// A registered linker instance shared the import's interface
    /// identifier but its version fell outside the import's
    /// WIT-spec compatibility range.
    #[error("import `{import}` requested version {requested:?}, available versions {available:?} are not compatible")]
    IncompatibleVersion {
        /// The name of the import whose version constraint failed.
        import: ExternalName,
        /// The version the import requested. `None` represents an
        /// unversioned import; pairing it with a versioned
        /// registration is rejected by the WIT compatibility rules.
        requested: Option<Version>,
        /// The versions of the candidate registrations the linker
        /// considered. An empty vector here can occur when an
        /// unversioned candidate is paired with a versioned
        /// import.
        available: Vec<Option<Version>>,
    },

    /// The import's shape requires host-item registration the
    /// polyfill does not yet support (any host function or host
    /// resource). The structured reason names which capability is
    /// missing.
    #[error("import `{import}` requires unsupported host registration: {reason}")]
    UnsupportedRegistration {
        /// The name of the import whose shape is not yet supported.
        import: ExternalName,
        /// Which capability the polyfill would need to satisfy this
        /// import (e.g. `"host function"`, `"host resource"`).
        reason: &'static str,
    },
}

/// The reason instantiation failed after linking succeeded.
///
/// Surfaced as the inner cause of [`Error::Instantiation`].
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum InstantiationError {
    /// The runtime substrate refused to instantiate the component,
    /// or the component's `start` function trapped. The underlying
    /// runtime cause is captured as `#[source]`. `anyhow::Error`
    /// appears here because the runtime substrate surfaces fallible
    /// operations as `anyhow::Result`.
    #[error("the runtime substrate failed to instantiate the component")]
    SubstrateFailure(#[source] anyhow::Error),

    /// The polyfill rejected the component's shape: an exported
    /// signature requires compound-valtype lift/lower (records,
    /// variants, lists, options, results, tuples, flags, enums,
    /// strings, or resource handles) that the polyfill defers.
    #[error("export `{export}` has a signature the polyfill does not yet implement: {reason}")]
    UnsupportedSignature {
        /// The name of the export whose signature was rejected.
        export: String,
        /// The structural reason the signature is rejected.
        reason: &'static str,
    },
}

/// A `Result` whose error variant is the polyfill's [`Error`].
pub type Result<T> = core::result::Result<T, Error>;
