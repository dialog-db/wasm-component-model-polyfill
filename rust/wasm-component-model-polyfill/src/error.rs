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

    /// The concurrency scheduler could not carry a driver through a
    /// turn. The carried [`SchedulerCause`] names which of the four
    /// ways this can happen occurred.
    #[error("scheduler error: {0}")]
    Scheduler(#[source] SchedulerCause),

    /// A guest broke one of the rules that govern waitables and
    /// waitable sets. The carried [`WaitableCause`] names which rule.
    /// Each is a trap in the reference.
    #[error("waitable error: {0}")]
    Waitable(#[source] WaitableCause),

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

    /// The host registered an item of one kind under a name the
    /// component imports as another kind: a function where an
    /// instance is imported, an instance where a module is, and so
    /// on. The kinds are named in Wasmtime's words.
    #[error("import `{import}`: expected {expected} found {found}")]
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
/// Carried by [`Error::Scheduler`]. Nothing produces this cause yet;
/// the scheduler, the `run_concurrent` entry, and the suspend seam
/// each produce it once they land.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum SchedulerCause {
    /// A driver went idle with nothing ready, no host task pending,
    /// and its condition unmet. The message is Wasmtime 48's deadlock
    /// trap, `Trap::AsyncDeadlock` in `wasmtime-environ`'s
    /// `src/trap_encoding.rs`, so the conformance corpus can match it
    /// by substring.
    #[error("deadlock detected: event loop cannot make further progress")]
    Deadlock,

    /// A task that must not block went idle while waiting. The
    /// message is Wasmtime 48's cannot-block trap,
    /// `Trap::CannotBlockSyncTask` in `wasmtime-environ`'s
    /// `src/trap_encoding.rs`, so the conformance corpus can match it
    /// by substring.
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
    /// blocking, but the target has no suspend provider to switch its
    /// stack. Unlike [`Error::Unsupported`], the feature itself is
    /// supported here; only the capability to serve it on this
    /// target is missing, and a host may want to branch on that
    /// distinction.
    #[error("blocking here requires a stack switch, but the target has no suspend provider")]
    StackSwitchNeeded,
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
    /// message is Wasmtime 48's trap,
    /// `Trap::WaitableSetDropHasWaiters` in `wasmtime-environ`'s
    /// `src/trap_encoding.rs`, so the conformance corpus can match it
    /// by substring.
    #[error("cannot drop waitable set with waiters")]
    SetHasWaiters,

    /// A guest dropped a subtask whose resolution had not been
    /// delivered, so the handles the call borrowed were still lent
    /// out. The message is Wasmtime 48's trap,
    /// `Trap::SubtaskDropNotResolved`, under the same rule as
    /// [`WaitableCause::SetHasWaiters`].
    #[error("cannot drop a subtask which has not yet resolved")]
    SubtaskNotResolved,

    /// A guest added a waitable to a waitable set while a thread was
    /// waiting on that waitable on its own, or waited on a waitable
    /// on its own while it was in a set. The message is Wasmtime 48's
    /// trap, `Trap::WaitableSyncAndAsync`, under the same rule as
    /// [`WaitableCause::SetHasWaiters`].
    #[error("waitable cannot be used synchronously while added to a waitable set")]
    SyncAndAsync,
}

/// A `Result` whose error variant is the polyfill's [`Error`].
pub type Result<T> = core::result::Result<T, Error>;

#[cfg(test)]
mod tests {
    use wasmtime_environ::Trap;

    use super::*;

    #[test]
    fn it_renders_the_deadlock_cause_as_wasmtime_48s_trap_message() {
        let err = Error::Scheduler(SchedulerCause::Deadlock);
        assert_eq!(
            err.to_string(),
            "scheduler error: deadlock detected: event loop cannot make further progress"
        );
    }

    #[test]
    fn it_renders_the_cannot_block_cause_as_wasmtime_48s_trap_message() {
        let err = Error::Scheduler(SchedulerCause::CannotBlock);
        assert_eq!(
            err.to_string(),
            "scheduler error: cannot block a synchronous task before returning"
        );
    }

    #[test]
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

    #[test]
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

    #[test]
    fn it_renders_the_recursive_driver_cause() {
        let err = Error::Scheduler(SchedulerCause::RecursiveDriver);
        assert_eq!(
            err.to_string(),
            "scheduler error: a driver was entered while another was inside a turn, or an accessor was used inside another accessor's closure"
        );
    }

    #[test]
    fn it_renders_the_stack_switch_needed_cause() {
        let err = Error::Scheduler(SchedulerCause::StackSwitchNeeded);
        assert_eq!(
            err.to_string(),
            "scheduler error: blocking here requires a stack switch, but the target has no suspend provider"
        );
    }

    #[test]
    fn it_keeps_stack_switch_needed_distinct_from_unsupported() {
        let stack_switch = Error::Scheduler(SchedulerCause::StackSwitchNeeded);
        let unsupported = Error::unsupported("stream<T>");
        assert!(!matches!(stack_switch, Error::Unsupported { .. }));
        assert!(matches!(unsupported, Error::Unsupported { .. }));
    }

    #[test]
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

    #[test]
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
}
