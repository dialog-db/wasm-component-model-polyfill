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

use crate::component::{ExternalName, FunctionType};
use crate::identifier::InterfaceIdentifier;
use crate::types::ValueType;

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
    ///
    /// The cause is carried behind a `Box` so `Error` itself stays
    /// small at the boundary; matching against the variant is
    /// unaffected and the inner [`LinkError`] is reachable through
    /// a deref or pattern-binding the box.
    #[error("link error: {0}")]
    Link(#[source] Box<LinkError>),

    /// The runtime substrate failed to instantiate a successfully
    /// linked component, or the polyfill rejected the component for
    /// a structural reason it intentionally defers
    /// (e.g. an exported signature that requires compound-valtype
    /// lift/lower).
    #[error("instantiation error: {0}")]
    Instantiation(#[source] Box<InstantiationError>),

    /// A host-side type does not unify with the corresponding
    /// component-side type. The carried [`TypeMismatch`] identifies
    /// the position the mismatch was observed at and the two types
    /// that disagreed.
    ///
    /// Surfaced in two places: registering a typed or untyped host
    /// function whose declared signature does not satisfy the
    /// component's import (caught at link time, before the
    /// registration is accepted), and calling a typed export whose
    /// declared signature does not satisfy the export's component-
    /// level type.
    #[error("type mismatch: {0}")]
    TypeMismatch(#[source] Box<TypeMismatch>),

    /// Lift or lower of a value across the canonical-ABI boundary
    /// failed. The carried [`AbiError`] identifies the position the
    /// failure was observed at (an argument index or the result
    /// slot), the value type involved, and the structured cause.
    #[error("canonical ABI error: {0}")]
    Abi(#[source] Box<AbiError>),

    /// The component uses a Component Model feature the polyfill
    /// does not implement yet. The feature is named so a caller can
    /// tell "not built yet" from "broken". Reaching this variant is
    /// never a bug in the caller's component.
    #[error("unsupported component feature: {feature}")]
    Unsupported {
        /// A short description of the unsupported feature, for
        /// example `locally-defined resources` or `stream<T>`.
        feature: String,
    },

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

impl Error {
    /// Build an [`Error::Unsupported`] naming `feature`.
    ///
    /// Workspace-internal; not re-exported by `lib.rs`.
    pub fn unsupported(feature: impl Into<String>) -> Self {
        Error::Unsupported {
            feature: feature.into(),
        }
    }

    /// Build an [`Error::Internal`] carrying `message`.
    ///
    /// Workspace-internal; not re-exported by `lib.rs`.
    pub fn internal(message: impl Into<String>) -> Self {
        Error::Internal {
            message: message.into(),
        }
    }
}

impl From<LinkError> for Error {
    fn from(value: LinkError) -> Self {
        Error::Link(Box::new(value))
    }
}

impl From<InstantiationError> for Error {
    fn from(value: InstantiationError) -> Self {
        Error::Instantiation(Box::new(value))
    }
}

impl From<TypeMismatch> for Error {
    fn from(value: TypeMismatch) -> Self {
        Error::TypeMismatch(Box::new(value))
    }
}

impl From<AbiError> for Error {
    fn from(value: AbiError) -> Self {
        Error::Abi(Box::new(value))
    }
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
    #[error(
        "import `{import}` requested version {requested:?}, available versions {available:?} are not compatible"
    )]
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

    /// A function handle was called with a [`Store`] other than the
    /// one its [`Instance`] was created in. An instance's core
    /// state lives in exactly one store; the runtime substrate
    /// cannot address it through another.
    ///
    /// [`Store`]: crate::Store
    /// [`Instance`]: crate::Instance
    #[error("the function handle belongs to an instance created in a different store")]
    WrongStore,
}

/// A type-mismatch report.
///
/// Carried by [`Error::TypeMismatch`] for the two situations the
/// polyfill checks structurally: a host function being registered
/// against a component import, and a typed export call's declared
/// signature being matched against the export's component-level
/// signature. The `position` names where in the surface the
/// mismatch was observed; the `expected` and `actual` types are the
/// polyfill's own data shapes — no upstream type appears here.
#[derive(Debug, Error)]
#[error("at {position}: expected {expected}, found {actual}")]
pub struct TypeMismatch {
    /// Where the mismatch was observed.
    pub position: TypeMismatchPosition,
    /// The polyfill's structured rendering of the type the position
    /// expected.
    pub expected: TypeRendering,
    /// The polyfill's structured rendering of the type the position
    /// actually saw.
    pub actual: TypeRendering,
}

/// Where in the surface a [`TypeMismatch`] was observed.
#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum TypeMismatchPosition {
    /// A host function was being registered against an
    /// interface-named component import. The interface and the item
    /// inside it are named.
    HostFunctionRegistration {
        /// The interface the registration is attached to.
        interface: InterfaceIdentifier,
        /// The item name inside the interface.
        item: String,
    },
    /// A host function was being registered against a plain-named
    /// component import.
    HostFunctionRegistrationPlain {
        /// The plain (kebab-case) import name the registration was
        /// attached to.
        name: String,
    },
    /// A typed export call asserted a signature against an export
    /// whose declared component-level signature does not match.
    TypedExportCall {
        /// The name the component declares the export under.
        export: String,
    },
    /// A typed export call's argument or result is the wrong shape
    /// for the slot it is being lowered or lifted into. This is the
    /// per-slot complement to [`Self::TypedExportCall`].
    TypedExportSlot {
        /// The name the component declares the export under.
        export: String,
        /// Which slot the mismatch occurred at.
        slot: AbiPosition,
    },
    /// The typed-conversion entry point on an export's [`Func`]
    /// rejected the requested Rust parameter tuple and return type:
    /// the export's declared component-level signature does not
    /// match the signature the caller asked for. The `expected` and
    /// `actual` [`TypeRendering::Function`]s on the enclosing
    /// [`TypeMismatch`] carry the component-side and Rust-side
    /// signatures, respectively.
    ///
    /// [`Func`]: crate::Func
    TypedConversion {
        /// The name the component declares the export under.
        export: String,
    },
}

impl core::fmt::Display for TypeMismatchPosition {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::HostFunctionRegistration { interface, item } => {
                write!(f, "host registration for `{interface}#{item}`")
            }
            Self::HostFunctionRegistrationPlain { name } => {
                write!(f, "host registration for `{name}`")
            }
            Self::TypedExportCall { export } => {
                write!(f, "typed export call for `{export}`")
            }
            Self::TypedExportSlot { export, slot } => {
                write!(f, "typed export call for `{export}` at {slot}")
            }
            Self::TypedConversion { export } => {
                write!(f, "typed conversion for export `{export}`")
            }
        }
    }
}

/// A polyfill-typed rendering of a [`ValueType`] or a
/// [`FunctionType`] for inclusion in a [`TypeMismatch`].
///
/// Two renderings exist so a function-vs-function mismatch can be
/// reported with the full signature on each side, while a value-vs-
/// value mismatch can be reported with the offending value type
/// only. Both shapes are owned, polyfill-typed data.
#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum TypeRendering {
    /// A value type, used when the mismatch is at a single slot.
    Value(ValueType),
    /// A function type, used when an entire signature disagrees.
    Function(FunctionType),
}

impl core::fmt::Display for TypeRendering {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Value(ty) => write!(f, "{ty:?}"),
            Self::Function(ty) => write!(f, "{ty:?}"),
        }
    }
}

/// A canonical-ABI lift or lower failure.
///
/// Carried by [`Error::Abi`]. The `position` names the slot the
/// failure occurred at; `valtype` carries the polyfill's value-type
/// shape involved, and `cause` is the structured reason.
#[derive(Debug, Error)]
#[error("at {position} (type {valtype:?}): {cause}")]
pub struct AbiError {
    /// Where the failure was observed.
    pub position: AbiPosition,
    /// The value type the lift or lower was processing.
    pub valtype: ValueType,
    /// The structured cause of the failure.
    #[source]
    pub cause: AbiCause,
}

/// Which slot a canonical-ABI failure was observed at.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum AbiPosition {
    /// The argument at the given index.
    Argument(usize),
    /// The function's result slot. The synchronous baseline admits
    /// at most one result.
    Result,
}

impl core::fmt::Display for AbiPosition {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Argument(i) => write!(f, "argument {i}"),
            Self::Result => write!(f, "result"),
        }
    }
}

/// The structured reason a canonical-ABI lift or lower failed.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum AbiCause {
    /// A pointer or length read out of the guest's memory addressed
    /// a region the memory does not own.
    #[error("out-of-bounds memory access at offset {offset} for {length} bytes")]
    OutOfBoundsMemory {
        /// The offset the access started at.
        offset: usize,
        /// The number of bytes the access requested.
        length: usize,
    },

    /// The guest exposed no `cabi_realloc` (or it was unreachable),
    /// but the lower path needed to allocate guest memory for the
    /// value.
    #[error("the export requires `cabi_realloc` for this value type, but none is available")]
    ReallocUnavailable,

    /// Calling the guest's `cabi_realloc` failed or returned a
    /// pointer that the polyfill could not validate against memory
    /// bounds.
    #[error("`cabi_realloc` invocation failed")]
    ReallocFailed(#[source] anyhow::Error),

    /// A guest-supplied byte sequence was not valid for the value
    /// type's encoding (e.g. an invalid UTF-8 string, an invalid
    /// `char`, a discriminant outside the declared range).
    #[error("invalid encoding for value type: {message}")]
    InvalidEncoding {
        /// A short human-readable description of what was malformed.
        message: String,
    },

    /// The host supplied a `Val` whose variant does not match the
    /// declared value type. Mirrors the link-time `TypeMismatch`,
    /// but caught at call time when the host hands an untyped value
    /// to lower.
    #[error("host value variant does not match declared value type")]
    HostValueMismatch,

    /// The valtype is one whose lift or lower the polyfill defers
    /// to a later PDD.
    #[error("lift/lower for this valtype is not yet implemented")]
    Unimplemented,

    /// A handle index does not address a live entry in the
    /// per-store handle table, or the host supplied a handle whose
    /// resource-type identity does not match the declared
    /// `own<T>` / `borrow<T>`.
    #[error("invalid resource handle: {reason}")]
    InvalidHandle {
        /// A short description of why the handle was rejected
        /// (out-of-range index, type-id mismatch, etc.).
        reason: String,
    },

    /// The component transferred ownership of a resource handle to
    /// the host, but no host registration carries the matching
    /// resource type identity. Typically observed when a host
    /// receives an `own<T>` it never registered a destructor for.
    #[error("no host registration matches the transferred resource type")]
    UnregisteredResourceType,

    /// The guest's `cabi_realloc` returned a pointer the polyfill
    /// cannot use: past the end of memory, or not aligned as asked.
    #[error("realloc return: {reason}")]
    ReallocReturn {
        /// What was wrong with the pointer, in Wasmtime's words.
        reason: String,
    },

    /// A host call returned with `borrow<T>` handles still
    /// outstanding. The canonical-ABI runtime-state rules forbid
    /// this; the count is the number of borrows the lift recorded
    /// without an offsetting drop at return.
    #[error("{count} borrow handles outstanding at host-call return")]
    OutstandingBorrows {
        /// The number of unreleased borrows.
        count: usize,
    },

    /// A failure surfaced by a lower-level component (e.g. the
    /// runtime substrate) while reading or writing memory. The
    /// underlying cause is captured as `#[source]`.
    #[error("substrate-level memory access failed")]
    SubstrateFailure(#[source] anyhow::Error),
}

/// A `Result` whose error variant is the polyfill's [`Error`].
pub type Result<T> = core::result::Result<T, Error>;
