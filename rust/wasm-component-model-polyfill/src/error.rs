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
use crate::internal::ErrorInternal;
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
        /// failure. The width is the parser's own: a component
        /// binary is addressed in 64 bits whatever the host is.
        offset: u64,
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
    /// import's compatibility range, or the import requires
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

    /// The concurrency scheduler could not carry a driver through a
    /// turn. The carried [`SchedulerCause`] names which of the six
    /// ways this can happen occurred.
    #[error("scheduler error: {0}")]
    Scheduler(#[source] SchedulerCause),

    /// A guest broke one of the rules that govern waitables and
    /// waitable sets. The carried [`WaitableCause`] names which rule.
    /// Each is a trap in the reference.
    #[error("waitable error: {0}")]
    Waitable(#[source] WaitableCause),

    /// A guest broke one of the rules that govern tasks and the
    /// built-ins that read a task's state. The carried [`TaskCause`]
    /// names which rule. Each is a trap in the reference.
    #[error("task error: {0}")]
    Task(#[source] TaskCause),

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

impl ErrorInternal for Error {
    fn unsupported(feature: impl Into<String>) -> Error {
        Error::Unsupported {
            feature: feature.into(),
        }
    }

    fn internal(message: impl Into<String>) -> Error {
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
    /// identifier or its compatibility range, or the
    /// registration that did match holds nothing under the name one
    /// of the import's items asks for.
    #[error("{}", UnresolvedContext { import, item })]
    UnresolvedImport {
        /// The name of the import the linker could not satisfy.
        import: ExternalName,
        /// The item inside an instance import that has no
        /// registration, or `None` when the import itself is the
        /// one nothing satisfies.
        item: Option<String>,
    },

    /// More than one registered linker instance was equally good a
    /// match for the import, and the version tie-break did not
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
    /// compatibility range.
    #[error(
        "import `{import}` requested version {requested:?}, available versions {available:?} are not compatible"
    )]
    IncompatibleVersion {
        /// The name of the import whose version constraint failed.
        import: ExternalName,
        /// The version the import requested. `None` represents an
        /// unversioned import; pairing it with a versioned
        /// registration is rejected by the resolver's version rules.
        requested: Option<Version>,
        /// The versions of the candidate registrations the linker
        /// considered. An empty vector here can occur when an
        /// unversioned candidate is paired with a versioned
        /// import.
        available: Vec<Option<Version>>,
    },

    /// The host registered an item of one kind under a name the
    /// component imports as another kind: a function where an
    /// instance is imported, an instance where a module is, and so
    /// on. The kinds are named in Wasmtime's words.
    #[error(
        "import `{import}`:{} expected {expected} found {found}",
        ItemContext(item)
    )]
    KindMismatch {
        /// The name of the import whose kind disagreed.
        import: ExternalName,
        /// The item inside an instance import whose kind disagreed,
        /// or `None` when the import itself did.
        item: Option<String>,
        /// The kind the component declares.
        expected: &'static str,
        /// The kind the host registered.
        found: &'static str,
    },

    /// An import whose WIT type is declared `async func` was
    /// satisfied by a synchronous registration, one made through
    /// `LinkerInstance::func_new` or `LinkerInstance::func_wrap`.
    ///
    /// Nothing in the Component Model requires this. A synchronous
    /// host function would serve an async-typed import correctly,
    /// because it resolves at once, and how a caller reaches the
    /// function is an axis of its own separate from the callee's
    /// type. The rule is Wasmtime's choice, and the polyfill follows
    /// it so that a host's registrations move between the two
    /// runtimes unchanged: a pairing one refuses is a pairing the
    /// other refuses, with the same text.
    ///
    /// The message is Wasmtime's, which is why it names
    /// `func_new_async`/`func_wrap_async` — entries Wasmtime offers
    /// and the polyfill does not. They appear because they are the
    /// other sync-style pair a host might have reached for, and the
    /// message says that neither pair is what an `async func` import
    /// wants.
    #[error(
        "import `{import}`:{} type mismatch with async: this import is declared `async func` in \
         WIT, but was satisfied with a sync-style host function (`func_new`/`func_wrap`, or \
         `func_new_async`/`func_wrap_async` — despite the name, these implement a \
         *sync*-WIT-typed function via blocking host code, not an `async func` import); use \
         `func_new_concurrent`/`func_wrap_concurrent` instead",
        ItemContext(item)
    )]
    SynchronousRegistrationForAsyncImport {
        /// The name of the import whose type is `async func`.
        import: ExternalName,
        /// The item inside an instance import whose registration
        /// disagreed, or `None` when the import itself is the
        /// function.
        item: Option<String>,
    },

    /// An import whose WIT type is a plain, non-`async` function was
    /// satisfied by a concurrent registration, one made through
    /// `LinkerInstance::func_new_concurrent` or
    /// `LinkerInstance::func_wrap_concurrent`.
    ///
    /// This is the other half of
    /// [`Self::SynchronousRegistrationForAsyncImport`], and is
    /// Wasmtime's choice in the same way: the polyfill refuses the
    /// pairing so that a host's registrations move between the two
    /// runtimes unchanged. The message is Wasmtime's, and names
    /// `func_new_async`/`func_wrap_async` for the same reason.
    #[error(
        "import `{import}`:{} type mismatch with async: this import's WIT type is a plain \
         (non-`async`) function, but was satisfied with \
         `func_new_concurrent`/`func_wrap_concurrent`, which is only for `async func`-typed \
         imports; use `func_new`/`func_wrap` (or `func_new_async`/`func_wrap_async` for \
         blocking host code) instead",
        ItemContext(item)
    )]
    ConcurrentRegistrationForSyncImport {
        /// The name of the import whose type is a plain function.
        import: ExternalName,
        /// The item inside an instance import whose registration
        /// disagreed, or `None` when the import itself is the
        /// function.
        item: Option<String>,
    },

    /// The core module registered for a module-typed import does not
    /// satisfy the module type the import declares: an export the
    /// type lists is missing or has the wrong type, or the module
    /// asks for an import the type does not list. The reason is in
    /// Wasmtime's wording.
    #[error("the module registered as `{item}` for import `{import}` has the wrong type: {reason}")]
    IncompatibleModule {
        /// The name of the import the module was registered for.
        import: ExternalName,
        /// The name the module was registered under: the import's
        /// own name, or the item's name inside an instance import.
        item: String,
        /// Why the module does not satisfy the declared type.
        reason: String,
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

/// The clause a [`LinkError`] message inserts between the import it
/// names and the reason the link failed, when the failure is an item
/// inside an instance import rather than the import itself.
///
/// Wasmtime reports such a failure as a chain: the import's name is
/// the outer context, an `instance export ... has the wrong type`
/// line sits under it, and the reason sits under that. The polyfill
/// carries the same three parts in one flat message, so a failure on
/// an interface import with several function items says which item
/// it was. An item-less failure renders nothing, leaving the message
/// exactly as it reads when the import itself is at fault.
///
/// The name is the item's qualified name: the nested instance items
/// walked from the import, joined with dots, and the item last.
struct ItemContext<'a>(&'a Option<String>);

impl core::fmt::Display for ItemContext<'_> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self.0 {
            Some(item) => write!(f, " instance export `{item}` has the wrong type:"),
            None => Ok(()),
        }
    }
}

/// The message [`LinkError::UnresolvedImport`] renders.
///
/// The cause itself — that nothing registered satisfies the import —
/// is one sentence whichever part of the import is unsatisfied, so
/// an import that nothing at all matches reads exactly as it always
/// has. When the linker did find a registration for the import and
/// the miss is an item inside it, the import's name and the
/// [`ItemContext`] clause are laid ahead of that sentence, in the
/// order the sibling causes put them: the import, then which of its
/// exports went wrong, then why.
struct UnresolvedContext<'a> {
    /// The name of the import nothing satisfies.
    import: &'a ExternalName,
    /// The item inside it that has no registration, if the miss is
    /// an item rather than the import.
    item: &'a Option<String>,
}

impl core::fmt::Display for UnresolvedContext<'_> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let Self { import, item } = self;
        if item.is_some() {
            write!(f, "import `{import}`:{} ", ItemContext(item))?;
        }
        write!(
            f,
            "no registered linker instance satisfies import `{import}`"
        )
    }
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
    /// one its [`Instance`] was created in, or a core item from one
    /// store was supplied as an import into another. An instance's
    /// core state lives in exactly one store; the runtime substrate
    /// cannot address it through another.
    ///
    /// [`Store`]: crate::Store
    /// [`Instance`]: crate::Instance
    #[error("the handle belongs to an instance created in a different store")]
    WrongStore,

    /// A core module was instantiated from the host with a different
    /// number of imports than it declares. [`Module::instantiate`]
    /// takes one value per declared import, in declaration order.
    ///
    /// [`Module::instantiate`]: crate::Module::instantiate
    #[error("the module declares {expected} imports, but {found} were supplied")]
    ImportCount {
        /// The number of imports the module declares.
        expected: usize,
        /// The number of values the host supplied.
        found: usize,
    },
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
    /// interface-named instance import. The interface and the item
    /// inside it are named.
    HostFunctionRegistration {
        /// The interface the registration is attached to.
        interface: InterfaceIdentifier,
        /// The item name inside the interface.
        item: String,
    },
    /// A host function was being registered in the root namespace:
    /// against a plain-named import, an item inside a plain-named
    /// instance import, or an import that is not an instance and
    /// carries an interface name.
    ///
    /// The `Plain` in the variant's name is historical. It once
    /// described the shape of the name the registration sat under,
    /// back when only a plain-named import resolved through the
    /// root; it now marks the root namespace itself, and `name` is
    /// as often an interface identifier as a plain name. The
    /// variant keeps its name so that host code matching on it goes
    /// on compiling.
    HostFunctionRegistrationPlain {
        /// The root-namespace name the registration was attached to,
        /// which is the import's own name as the component writes
        /// it.
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
///
/// A few failures at a crossing process no value type: the
/// scope-exit rule of a borrow is one, and it fails a call that
/// returns nothing as readily as one that returns something. Those
/// carry no `valtype`, and the rendering leaves the type out rather
/// than naming one they were not processing.
#[derive(Debug, Error)]
#[error("at {position}{}: {cause}", type_label(valtype))]
pub struct AbiError {
    /// Where the failure was observed.
    pub position: AbiPosition,
    /// The value type the lift or lower was processing, when it was
    /// processing one.
    pub valtype: Option<ValueType>,
    /// The structured cause of the failure.
    #[source]
    pub cause: AbiCause,
}

/// The ` (type T)` an [`AbiError`] renders between its position and
/// its cause, and nothing at all when the failure names no value
/// type.
fn type_label(valtype: &Option<ValueType>) -> String {
    match valtype {
        Some(valtype) => format!(" (type {valtype:?})"),
        None => String::new(),
    }
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

    /// A handle index does not address a live entry in the handle
    /// table it was presented against, or the host supplied a handle
    /// whose resource-type identity does not match the declared
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
    ///
    /// Every failure with this cause names the resource type at
    /// issue in the error's `valtype`, and names it as an `own<T>`
    /// or a `borrow<T>`: lowering a handle names the handle slot the
    /// component declared, and a host mint against an identity the
    /// calling instance holds no table for names the type the store
    /// knows that identity by. It names nothing only when there is
    /// no name to give — an identity no registration and no
    /// instantiation of the store ever introduced.
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

    /// The crossing runs under a data model whose ABI strategy the
    /// polyfill does not implement. A component that declares such a
    /// data model is refused at translation with
    /// [`Error::Unsupported`], so a crossing reaches this cause only
    /// when it was built against those options directly.
    #[error("the canonical-ABI strategy for this data model is not implemented")]
    UnsupportedDataModel,
}

/// The structured reason the concurrency scheduler failed to carry a
/// driver through a turn.
///
/// Carried by [`Error::Scheduler`]. The scheduler, the
/// `run_concurrent` entry, and the suspend seam each raise the cause
/// that names what they met.
/// [`SchedulerCause::ReentrantHostCall`] is the odd one: it is not
/// the scheduler's judgement of anything but the browser backend's
/// refusal of a call the guest made, read back off the failure and
/// reported here so that a host branching on a scheduler cause finds
/// it beside the others.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum SchedulerCause {
    /// A driver, or a suspension that fell back to a nested turn,
    /// went idle with nothing ready, no host task pending, no
    /// synchronous call left to return, and its condition unmet. An
    /// idle store that still holds such a call — an instance still
    /// carrying may-not-suspend — fails with
    /// [`SchedulerCause::CannotBlock`] instead. The message is
    /// Wasmtime's deadlock trap, `Trap::AsyncDeadlock` in
    /// `wasmtime-environ`'s `src/trap_encoding.rs`, so the
    /// conformance corpus can match it by substring.
    #[error("deadlock detected: event loop cannot make further progress")]
    Deadlock,

    /// A task that must not block went idle while waiting, or a
    /// task that may block found the store idle while an instance
    /// still carried may-not-suspend — some synchronous call had not
    /// returned. Wasmtime reports this trap in the second case too:
    /// the callee blocking forever is that caller failing to return,
    /// so the cause names the caller's rule. The message is
    /// Wasmtime's cannot-block trap, `Trap::CannotBlockSyncTask` in
    /// `wasmtime-environ`'s `src/trap_encoding.rs`, so the
    /// conformance corpus can match it by substring.
    #[error("cannot block a synchronous task before returning")]
    CannotBlock,

    /// A driver was entered while another driver of the same store
    /// was already inside a turn, or an accessor was used from inside
    /// another accessor's closure.
    #[error(
        "a driver was entered while another was inside a turn, or an accessor was used inside another accessor's closure"
    )]
    RecursiveDriver,

    /// A guest thread blocked at a point the reference permits
    /// blocking, the store still held work the block could not
    /// reach, and the target has no suspend provider to switch its
    /// stack. Unlike [`Error::Unsupported`], the feature itself is
    /// supported here; only the capability to serve it on this
    /// target is missing, and a host may want to branch on that
    /// distinction. A block that the store went idle under fails
    /// with [`SchedulerCause::Deadlock`] instead, because nothing
    /// left in the store could have met its condition — or with
    /// [`SchedulerCause::CannotBlock`] when an instance still
    /// carried may-not-suspend at idle, because a synchronous call
    /// had yet to return.
    #[error("blocking here requires a stack switch, but the target has no suspend provider")]
    StackSwitchNeeded,

    /// An accessor reached for its store where no poll of that store
    /// was running: outside every poll, or from inside a poll of
    /// another store. The accessor is a token — it carries a store's
    /// identity and borrows nothing — and the store it names is
    /// reachable only while a poll of that store has left the
    /// store's context in the thread's slot. Wasmtime panics on the
    /// same misuse; the polyfill answers with this cause, because
    /// reaching through an accessor already returns a result. A
    /// reach made from inside another reach of the same store, where
    /// a poll is running but the store is out on loan, fails with
    /// [`SchedulerCause::RecursiveDriver`] instead.
    #[error("an accessor reached its store outside a poll of that store")]
    StoreNotInPoll,

    /// Guest work called a host function a call of which was still
    /// on the stack, on a target whose host functions cannot be
    /// entered twice.
    ///
    /// A blocking built-in that finds no suspend provider runs a
    /// nested turn from inside the lowered import the guest called,
    /// so that import's host function is still on the stack while
    /// the turn runs. An item of that turn which calls the same
    /// import is therefore a second call of the same host function.
    /// A destructor reaches the same place with no turn of any kind
    /// in it: `resource.drop` is a host function, and it runs the
    /// destructor from inside itself, so a destructor that drops a
    /// second handle of its own resource type in the same instance
    /// calls that host function again. The native backend serves
    /// either, because a native engine enters a host function at any
    /// depth. The browser backend refuses it: the browser has one
    /// JavaScript function object per host function, and the
    /// arguments and results of a call belong to that call alone.
    /// The refusal reaches the guest as a trap and the guest's caller
    /// as this cause, so a host reading it knows the component is
    /// sound and the target is what could not run it.
    #[error(
        "this target cannot call a host function while a call of the same host function is still on the stack, and guest work did"
    )]
    ReentrantHostCall,
}

/// The structured reason a waitable operation failed.
///
/// Carried by [`Error::Waitable`]. Each cause is a trap in the
/// reference, so a built-in that meets one fails the guest's call.
/// Nothing produces these yet; the built-ins of the concurrency
/// features produce them once they land.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum WaitableCause {
    /// A guest dropped a waitable set that still held waitables. The
    /// reference traps on the same condition; Wasmtime has no trap of
    /// its own for it, so the message is the polyfill's own, written
    /// to read like the one beside it.
    #[error("cannot drop waitable set with waitables in it")]
    SetHasWaitables,

    /// A guest dropped a waitable set a thread was waiting on. The
    /// message is Wasmtime's trap,
    /// `Trap::WaitableSetDropHasWaiters` in `wasmtime-environ`'s
    /// `src/trap_encoding.rs`, so the conformance corpus can match it
    /// by substring.
    #[error("cannot drop waitable set with waiters")]
    SetHasWaiters,

    /// A guest dropped a subtask whose resolution had not been
    /// delivered, so the handles the call borrowed were still lent
    /// out. The message is Wasmtime's trap,
    /// `Trap::SubtaskDropNotResolved`, under the same rule as
    /// [`WaitableCause::SetHasWaiters`].
    #[error("cannot drop a subtask which has not yet resolved")]
    SubtaskNotResolved,

    /// A guest added a waitable to a waitable set while a thread was
    /// waiting on that waitable on its own, or waited on a waitable
    /// on its own while it was in a set. The message is Wasmtime's
    /// trap, `Trap::WaitableSyncAndAsync`, under the same rule as
    /// [`WaitableCause::SetHasWaiters`].
    #[error("waitable cannot be used synchronously while added to a waitable set")]
    SyncAndAsync,
}

/// The structured reason a task operation failed.
///
/// Carried by [`Error::Task`]. Each cause is a trap in the reference,
/// so a built-in that meets one fails the guest's call. Nothing
/// produces these yet; the task built-ins produce them once they
/// land.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum TaskCause {
    /// The implicit thread of a task exited and the task had not
    /// returned. The message is Wasmtime 48's trap,
    /// `Trap::NoAsyncResult` in `wasmtime-environ`'s
    /// `src/trap_encoding.rs`, so the conformance corpus can match it
    /// by substring.
    #[error("async-lifted export failed to produce a result")]
    NoResult,

    /// A guest ran `task.return` on a task that was already resolved.
    /// The message is Wasmtime 48's trap,
    /// `Trap::TaskCancelOrReturnTwice`, under the same rule as
    /// [`TaskCause::NoResult`].
    #[error("`task.return` or `task.cancel` called more than once for current task")]
    ReturnedTwice,

    /// The result type or the options of a `task.return` differ from
    /// the ones the task's function was lifted with. The message
    /// opens with Wasmtime 48's trap, `Trap::TaskReturnInvalid`,
    /// under the same rule as [`TaskCause::NoResult`], and names the
    /// comparison that failed after it: the trap says only that one
    /// of the three did.
    #[error("{}: {kind}", TaskCause::RETURN_MISMATCH)]
    ReturnMismatch {
        /// Which of the three comparisons the built-in failed.
        kind: ReturnMismatchKind,
    },

    /// A guest ran `task.return` in a task whose lift is not `async`.
    /// The reference traps on the same condition; Wasmtime has no
    /// trap of its own for it, so the message is the polyfill's own,
    /// written to read like the ones beside it.
    #[error("`task.return` called for a task that was not lifted `async`")]
    ReturnFromSynchronousTask,

    /// A callback returned a status word whose code is above two. The
    /// message is Wasmtime 48's trap, `Trap::UnsupportedCallbackCode`,
    /// under the same rule as [`TaskCause::NoResult`].
    #[error("unsupported callback code")]
    UnsupportedCallbackCode,

    /// A backpressure built-in took the counter out of its range, in
    /// either direction. The message is Wasmtime 48's trap,
    /// `Trap::BackpressureOverflow`, under the same rule as
    /// [`TaskCause::NoResult`].
    #[error("backpressure counter overflow")]
    BackpressureOverflow,

    /// A built-in that reads the may-leave flag ran while the flag
    /// was clear, from a realloc or a post-return. The message is
    /// Wasmtime 48's trap, `Trap::CannotLeaveComponent`, under the
    /// same rule as [`TaskCause::NoResult`].
    #[error("cannot leave component instance")]
    CannotLeave,
}

impl TaskCause {
    /// Wasmtime's whole message for a `task.return` mismatch, which
    /// the rendering of [`TaskCause::ReturnMismatch`] opens with. The
    /// conformance corpus matches the trap by substring, so the
    /// prefix has to stay as Wasmtime writes it; the comparison that
    /// failed follows it.
    const RETURN_MISMATCH: &'static str =
        "invalid `task.return` signature and/or options for current task";
}

/// Which of the three comparisons a `task.return` failed.
///
/// The built-in compares its result type, its string encoding, and
/// its memory against the ones the task's function was lifted with,
/// and a difference in any one of them traps. Wasmtime's trap says
/// only that one of the three differed; the polyfill carries which,
/// so a reader of the trap — and a test of it — does not have to
/// guess.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum ReturnMismatchKind {
    /// The result type the built-in was declared with is not the
    /// result of the function the task is a call into. The
    /// comparison is structural.
    ResultType,
    /// The string encoding the built-in's options declare is not the
    /// one the task's lift declared.
    StringEncoding,
    /// The memory the built-in's options name is not the task's
    /// memory. Options that name no memory pass the comparison, so
    /// this is only ever a built-in that names one.
    Memory,
}

impl core::fmt::Display for ReturnMismatchKind {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::ResultType => write!(f, "the result type is not the task's"),
            Self::StringEncoding => write!(f, "the string encoding is not the task's"),
            Self::Memory => write!(f, "the memory is not the task's"),
        }
    }
}

/// A `Result` whose error variant is the polyfill's [`Error`].
pub type Result<T> = core::result::Result<T, Error>;

#[cfg(test)]
mod tests {
    use wasmtime_environ::Trap;

    use super::*;
    use crate::types::PrimitiveType;

    /// A [`LinkError`] that carries an item name renders it between
    /// the import it names and the reason, and one that does not
    /// renders the reason straight after the import.
    #[wcmp_macros::test]
    fn it_renders_the_item_of_a_link_failure_inside_an_instance_import() {
        let import = ExternalName::Plain("host".to_owned());
        let kind_mismatch = |item: Option<&str>| LinkError::KindMismatch {
            import: import.clone(),
            item: item.map(str::to_owned),
            expected: "func",
            found: "resource",
        };
        assert_eq!(
            kind_mismatch(Some("f")).to_string(),
            "import `host`: instance export `f` has the wrong type: expected func found resource"
        );
        assert_eq!(
            kind_mismatch(None).to_string(),
            "import `host`: expected func found resource"
        );

        // A nested item is named by the path walked to it.
        assert_eq!(
            kind_mismatch(Some("inner.f")).to_string(),
            "import `host`: instance export `inner.f` has the wrong type: expected func found \
             resource"
        );

        // The two registration-kind causes render the item the same
        // way, ahead of their own (long) reason, and read without
        // the clause when the import itself is the function.
        let sync = |item: Option<&str>| LinkError::SynchronousRegistrationForAsyncImport {
            import: import.clone(),
            item: item.map(str::to_owned),
        };
        assert_eq!(
            sync(Some("answer")).to_string(),
            "import `host`: instance export `answer` has the wrong type: type mismatch with \
             async: this import is declared `async func` in WIT, but was satisfied with a \
             sync-style host function (`func_new`/`func_wrap`, or \
             `func_new_async`/`func_wrap_async` — despite the name, these implement a \
             *sync*-WIT-typed function via blocking host code, not an `async func` import); use \
             `func_new_concurrent`/`func_wrap_concurrent` instead"
        );
        assert_eq!(
            sync(None).to_string(),
            "import `host`: type mismatch with async: this import is declared `async func` in \
             WIT, but was satisfied with a sync-style host function (`func_new`/`func_wrap`, or \
             `func_new_async`/`func_wrap_async` — despite the name, these implement a \
             *sync*-WIT-typed function via blocking host code, not an `async func` import); use \
             `func_new_concurrent`/`func_wrap_concurrent` instead"
        );

        let concurrent = |item: Option<&str>| LinkError::ConcurrentRegistrationForSyncImport {
            import: import.clone(),
            item: item.map(str::to_owned),
        };
        assert_eq!(
            concurrent(Some("answer")).to_string(),
            "import `host`: instance export `answer` has the wrong type: type mismatch with \
             async: this import's WIT type is a plain (non-`async`) function, but was satisfied \
             with `func_new_concurrent`/`func_wrap_concurrent`, which is only for `async \
             func`-typed imports; use `func_new`/`func_wrap` (or \
             `func_new_async`/`func_wrap_async` for blocking host code) instead"
        );
        assert_eq!(
            concurrent(None).to_string(),
            "import `host`: type mismatch with async: this import's WIT type is a plain \
             (non-`async`) function, but was satisfied with \
             `func_new_concurrent`/`func_wrap_concurrent`, which is only for `async func`-typed \
             imports; use `func_new`/`func_wrap` (or `func_new_async`/`func_wrap_async` for \
             blocking host code) instead"
        );
    }

    /// An unresolved import that knows which item inside an instance
    /// import went unsatisfied names it through the same clause the
    /// sibling causes use, and one that does not reads exactly as it
    /// always has.
    #[wcmp_macros::test]
    fn it_renders_the_item_of_an_unresolved_import_inside_an_instance_import() {
        let import = ExternalName::Plain("host".to_owned());
        let unresolved = |item: Option<&str>| LinkError::UnresolvedImport {
            import: import.clone(),
            item: item.map(str::to_owned),
        };
        assert_eq!(
            unresolved(None).to_string(),
            "no registered linker instance satisfies import `host`"
        );
        assert_eq!(
            unresolved(Some("f")).to_string(),
            "import `host`: instance export `f` has the wrong type: no registered linker \
             instance satisfies import `host`"
        );

        // A nested item is named by the path walked to it.
        assert_eq!(
            unresolved(Some("inner.f")).to_string(),
            "import `host`: instance export `inner.f` has the wrong type: no registered linker \
             instance satisfies import `host`"
        );
    }

    #[wcmp_macros::test]
    fn it_renders_an_abi_error_with_a_type_label_only_when_it_carries_a_value_type() {
        for (error, rendered) in [
            (
                AbiError {
                    position: AbiPosition::Argument(1),
                    valtype: Some(ValueType::Primitive(PrimitiveType::U32)),
                    cause: AbiCause::HostValueMismatch,
                },
                "canonical ABI error: at argument 1 (type Primitive(U32)): host value \
                 variant does not match declared value type",
            ),
            (
                AbiError {
                    position: AbiPosition::Result,
                    valtype: None,
                    cause: AbiCause::OutstandingBorrows { count: 1 },
                },
                "canonical ABI error: at result: 1 borrow handles outstanding at \
                 host-call return",
            ),
        ] {
            assert_eq!(Error::from(error).to_string(), rendered);
        }
    }

    #[wcmp_macros::test]
    fn it_renders_the_deadlock_cause_as_the_trap_message() {
        let err = Error::Scheduler(SchedulerCause::Deadlock);
        assert_eq!(
            err.to_string(),
            "scheduler error: deadlock detected: event loop cannot make further progress"
        );
    }

    #[wcmp_macros::test]
    fn it_renders_the_cannot_block_cause_as_the_trap_message() {
        let err = Error::Scheduler(SchedulerCause::CannotBlock);
        assert_eq!(
            err.to_string(),
            "scheduler error: cannot block a synchronous task before returning"
        );
    }

    #[wcmp_macros::test]
    fn it_pins_the_deadlock_cause_to_the_trap_wasmtime_environ_renders() {
        let trap = Trap::AsyncDeadlock.to_string();
        let cause = SchedulerCause::Deadlock.to_string();
        assert!(
            trap.ends_with(&cause),
            "`Trap::AsyncDeadlock` now renders as {trap:?}, which no longer ends with \
             the `SchedulerCause::Deadlock` message {cause:?}; the conformance corpus \
             matches this trap by substring, so the message has to follow the trap"
        );
    }

    #[wcmp_macros::test]
    fn it_pins_the_cannot_block_cause_to_the_trap_wasmtime_environ_renders() {
        let trap = Trap::CannotBlockSyncTask.to_string();
        let cause = SchedulerCause::CannotBlock.to_string();
        assert!(
            trap.ends_with(&cause),
            "`Trap::CannotBlockSyncTask` now renders as {trap:?}, which no longer ends \
             with the `SchedulerCause::CannotBlock` message {cause:?}; the conformance \
             corpus matches this trap by substring, so the message has to follow the trap"
        );
    }

    #[wcmp_macros::test]
    fn it_renders_the_recursive_driver_cause() {
        let err = Error::Scheduler(SchedulerCause::RecursiveDriver);
        assert_eq!(
            err.to_string(),
            "scheduler error: a driver was entered while another was inside a turn, or an accessor was used inside another accessor's closure"
        );
    }

    #[wcmp_macros::test]
    fn it_renders_the_stack_switch_needed_cause() {
        let err = Error::Scheduler(SchedulerCause::StackSwitchNeeded);
        assert_eq!(
            err.to_string(),
            "scheduler error: blocking here requires a stack switch, but the target has no suspend provider"
        );
    }

    #[wcmp_macros::test]
    fn it_renders_the_store_not_in_poll_cause() {
        let err = Error::Scheduler(SchedulerCause::StoreNotInPoll);
        assert_eq!(
            err.to_string(),
            "scheduler error: an accessor reached its store outside a poll of that store"
        );
    }

    #[wcmp_macros::test]
    fn it_keeps_stack_switch_needed_distinct_from_unsupported() {
        let stack_switch = Error::Scheduler(SchedulerCause::StackSwitchNeeded);
        let unsupported = Error::unsupported("stream<T>");
        assert!(!matches!(stack_switch, Error::Unsupported { .. }));
        assert!(matches!(unsupported, Error::Unsupported { .. }));
    }

    #[wcmp_macros::test]
    fn it_renders_the_waitable_set_drop_causes() {
        assert_eq!(
            Error::Waitable(WaitableCause::SetHasWaitables).to_string(),
            "waitable error: cannot drop waitable set with waitables in it"
        );
        assert_eq!(
            Error::Waitable(WaitableCause::SetHasWaiters).to_string(),
            "waitable error: cannot drop waitable set with waiters"
        );
    }

    #[wcmp_macros::test]
    fn it_pins_the_waitable_causes_to_the_traps_wasmtime_environ_renders() {
        for (trap, cause) in [
            (
                Trap::WaitableSetDropHasWaiters,
                WaitableCause::SetHasWaiters.to_string(),
            ),
            (
                Trap::SubtaskDropNotResolved,
                WaitableCause::SubtaskNotResolved.to_string(),
            ),
            (
                Trap::WaitableSyncAndAsync,
                WaitableCause::SyncAndAsync.to_string(),
            ),
        ] {
            let rendered = trap.to_string();
            assert!(
                rendered.ends_with(&cause),
                "{trap:?} now renders as {rendered:?}, which no longer ends with the \
                 `WaitableCause` message {cause:?}; the conformance corpus matches these \
                 traps by substring, so the messages have to follow the traps"
            );
        }
    }

    #[wcmp_macros::test]
    fn it_renders_the_task_causes() {
        for (cause, rendered) in [
            (
                TaskCause::NoResult,
                "task error: async-lifted export failed to produce a result",
            ),
            (
                TaskCause::ReturnedTwice,
                "task error: `task.return` or `task.cancel` called more than once for current task",
            ),
            (
                TaskCause::ReturnMismatch {
                    kind: ReturnMismatchKind::ResultType,
                },
                "task error: invalid `task.return` signature and/or options for current task: \
                 the result type is not the task's",
            ),
            (
                TaskCause::ReturnMismatch {
                    kind: ReturnMismatchKind::StringEncoding,
                },
                "task error: invalid `task.return` signature and/or options for current task: \
                 the string encoding is not the task's",
            ),
            (
                TaskCause::ReturnMismatch {
                    kind: ReturnMismatchKind::Memory,
                },
                "task error: invalid `task.return` signature and/or options for current task: \
                 the memory is not the task's",
            ),
            (
                TaskCause::ReturnFromSynchronousTask,
                "task error: `task.return` called for a task that was not lifted `async`",
            ),
            (
                TaskCause::UnsupportedCallbackCode,
                "task error: unsupported callback code",
            ),
            (
                TaskCause::BackpressureOverflow,
                "task error: backpressure counter overflow",
            ),
            (
                TaskCause::CannotLeave,
                "task error: cannot leave component instance",
            ),
        ] {
            assert_eq!(Error::Task(cause).to_string(), rendered);
        }
    }

    #[wcmp_macros::test]
    fn it_pins_the_task_causes_to_the_traps_wasmtime_environ_renders() {
        for (trap, cause) in [
            (Trap::NoAsyncResult, TaskCause::NoResult.to_string()),
            (
                Trap::TaskCancelOrReturnTwice,
                TaskCause::ReturnedTwice.to_string(),
            ),
            // The rendering of the mismatch cause names the
            // comparison that failed after the trap's own words, so
            // what the trap has to match is the prefix the cause
            // opens with.
            (
                Trap::TaskReturnInvalid,
                TaskCause::RETURN_MISMATCH.to_owned(),
            ),
            (
                Trap::UnsupportedCallbackCode,
                TaskCause::UnsupportedCallbackCode.to_string(),
            ),
            (
                Trap::BackpressureOverflow,
                TaskCause::BackpressureOverflow.to_string(),
            ),
            (
                Trap::CannotLeaveComponent,
                TaskCause::CannotLeave.to_string(),
            ),
        ] {
            let rendered = trap.to_string();
            assert!(
                rendered.ends_with(&cause),
                "{trap:?} now renders as {rendered:?}, which no longer ends with the \
                 `TaskCause` message {cause:?}; the conformance corpus matches these \
                 traps by substring, so the messages have to follow the traps"
            );
        }
    }
}
