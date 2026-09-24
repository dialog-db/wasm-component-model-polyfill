//! The executor's owned, target-agnostic IR.
//!
//! The polyfill's executor consumes a small, polyfill-owned shape
//! rather than `wasmtime_environ::component::Component` directly, so
//! the executor never names a translator type. One translator,
//! `wasmtime_environ`'s component `Translator`, runs on every target
//! and is projected into [`ExecutorIr`].

use std::collections::HashMap;
use std::sync::Arc;

use crate::abi::layout::FlatType;
use crate::abi::signature::Signature;
use crate::component::ExternalName;
use crate::concurrency::{EndKind, LowerKind};
use crate::module::Module;
use crate::types::{ResourceType, ValueType};

/// The executor's IR for a single parsed component.
///
/// All fields are owned and indexed in declaration order. The
/// executor walks `initializers` to drive substrate-level
/// instantiation, then walks `exports` to expose component-level
/// function handles.
///
/// Each `num_runtime_*` field names the size of the slab the
/// initializers populate. Slot ordering matches the order in which
/// the corresponding `Extract*` initializer produces its entry, and
/// `CanonOptions` names an entry in those slabs by index.
///
/// `num_component_instances` is the translator's per-instantiation
/// index space, not a slab an initializer fills: it sizes the
/// instance records, their handle tables, and their `may_leave`
/// flags, which are built before the initializer walk begins.
/// `CanonOptions::instance` and `ImportSource::InstanceFlags` name
/// an instance by that index.
pub struct ExecutorIr {
    /// One entry per `(core module ...)` section, in declaration
    /// order.
    pub modules: Box<[ModuleEntry]>,
    /// The flattened orchestration sequence: the actions the
    /// executor performs to produce the runtime state of an
    /// instantiated component.
    pub initializers: Box<[Initializer]>,
    /// The component's function exports, in declaration order,
    /// including the functions nested inside instance-typed exports.
    pub exports: Box<[ExportSpec]>,
    /// The path of every instance-typed export, at any depth, in
    /// declaration order: the names from the root of the export tree
    /// down to the instance itself. An instance is listed whether or
    /// not it holds a function.
    pub instance_exports: Box<[Box<[ExternalName]>]>,
    /// The component's module-typed exports, at any depth, in
    /// declaration order.
    pub module_exports: Box<[ModuleExportSpec]>,
    /// One entry per trampoline the component requires, in the
    /// order [`Trampoline`]s are emitted by the upstream translator.
    /// Each entry names what the trampoline does — lower a host
    /// import, run a `resource.drop` intrinsic, etc.
    pub trampoline_specs: Box<[TrampolineSpec]>,
    /// Per-resource metadata, indexed by the polyfill's own
    /// resource index. Used by [`TrampolineSpec::ResourceDrop`] and
    /// friends to resolve which host-resource registration the
    /// trampoline dispatches into.
    pub resources: Box<[ResourceSpec]>,
    /// One entry per resource table of the component, by the
    /// translator's table index: the translator names one such table
    /// for each pair of a component instance and a resource type,
    /// which the polyfill resolves to the single table that instance
    /// keeps. `None` marks an abstract table, one that no concrete
    /// instance holds.
    pub resource_tables: Box<[Option<ResourceTableSpec>]>,
    /// Maps each runtime-instance position (the index a
    /// [`CoreInstanceExport`] uses) to the polyfill's `modules`
    /// slot the instance was instantiated against, or `None` for an
    /// instance of an imported module, whose exports the translator
    /// names rather than indexes. The runtime-instance position is
    /// the count of preceding [`Initializer::InstantiateModule`] and
    /// [`Initializer::InstantiateImportedModule`] directives, so the
    /// n-th entry here describes the n-th instance the executor
    /// builds.
    pub runtime_instance_to_module: Box<[Option<usize>]>,
    /// The number of runtime memory slots `Initializer::ExtractMemory`
    /// populates. Slot 0 corresponds to the first directive, slot 1
    /// to the second, and so on.
    pub num_runtime_memories: usize,
    /// The number of runtime realloc slots
    /// `Initializer::ExtractRealloc` populates.
    pub num_runtime_reallocs: usize,
    /// The number of runtime post-return slots
    /// `Initializer::ExtractPostReturn` populates.
    pub num_runtime_post_returns: usize,
    /// The number of runtime callback slots
    /// `Initializer::ExtractCallback` populates.
    pub num_runtime_callbacks: usize,
    /// The number of component instances the component contains,
    /// counting nested components. Each carries a `may_leave` flags
    /// global that adapter modules import through
    /// [`ImportSource::InstanceFlags`].
    pub num_component_instances: usize,
}

/// One core module pre-translated to the runtime layer.
///
/// Carries the compiled module the executor instantiates against,
/// whose import list the executor pairs with each instantiation's
/// [`ImportSource`] list, and an inverted export table (so an
/// `EntityIndex`-keyed lookup produces a name the runtime-layer
/// instance's `get_export` accepts).
pub struct ModuleEntry {
    /// The compiled module. Constructed at translation time from the
    /// core module's binary slice; the same handle a module-typed
    /// export hands to the host.
    pub module: Module,
    /// Inverted export table: maps each export's slot
    /// (entity-index) to the name the module declares it under.
    /// Used when a component's [`CoreSourceItem::Index`] resolves
    /// against this module.
    pub entity_to_name: HashMap<EntityIndex, String>,
}

/// One component-level module export the executor exposes to the
/// caller through [`crate::Instance::get_module`] or through the
/// [`crate::Instance::exports`] navigator.
pub struct ModuleExportSpec {
    /// The leaf name the export is declared under.
    pub name: String,
    /// The names of the instance-typed exports that enclose this
    /// module, from the root of the export tree inward, or empty for
    /// a root-level module export.
    pub path: Box<[ExternalName]>,
    /// Where the module comes from.
    pub source: ModuleSource,
}

/// Where a core module the component names comes from.
pub enum ModuleSource {
    /// A module the component binary contains: an index into
    /// [`ExecutorIr::modules`].
    Static(usize),
    /// A module the component imports, resolved at instantiation
    /// time against the linker's registered modules.
    Import {
        /// Index into the polyfill component's imports
        /// (`Component::imports`): the import that is the module, or
        /// the imported instance that holds it.
        import_index: usize,
        /// The names from the imported instance down to the module,
        /// one per nesting level. Empty when the import is itself
        /// the module.
        path: Box<[String]>,
    },
}

/// The kind of a core module's exported entity. Maps 1:1 onto
/// `wasmtime_environ::EntityIndex` so the translator can project
/// from it without a name-mapping table.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum EntityIndex {
    /// A function index inside the module.
    Function(u32),
    /// A table index inside the module.
    Table(u32),
    /// A memory index inside the module.
    Memory(u32),
    /// A global index inside the module.
    Global(u32),
    /// A tag index inside the module.
    Tag(u32),
}

/// One step in the component's flattened orchestration sequence.
pub enum Initializer {
    /// Instantiate a previously-declared core module with imports
    /// resolved from the component's prior runtime state.
    InstantiateModule {
        /// Index into [`ExecutorIr::modules`].
        module_index: usize,
        /// The component instance the core instance belongs to, by
        /// the translator's per-instantiation index. `None` for an
        /// adapter module, which belongs to no component instance.
        /// The core module's `start` function runs inside
        /// instantiation, and it is a call into that instance, so it
        /// runs in a task of it.
        component_instance: Option<usize>,
        /// One [`ImportSource`] per declared import of the module,
        /// in the same declaration order
        /// [`ModuleEntry::imports`] enumerates.
        imports: Box<[ImportSource]>,
    },

    /// Instantiate a core module the component imports, with the
    /// imports the component supplies by name. The registered module
    /// decides the order it takes them in.
    InstantiateImportedModule {
        /// Where the module comes from; always [`ModuleSource::Import`].
        source: ModuleSource,
        /// The component instance the core instance belongs to, by
        /// the translator's per-instantiation index, named as the
        /// `InstantiateModule` directive above names it. A
        /// host-supplied module has a `start` function like any
        /// other, so it runs in a task of that instance too.
        component_instance: Option<usize>,
        /// The items the component supplies, by two-level name.
        imports: Box<[NamedImportSource]>,
    },

    /// Extract a core memory from a previously-instantiated core
    /// instance and bind it to the next runtime-memory slot. The
    /// slot is `0` for the first such directive in
    /// [`ExecutorIr::initializers`], `1` for the second, and so on.
    ExtractMemory {
        /// The runtime-memory slot this directive populates.
        slot: usize,
        /// Where the underlying core memory comes from.
        source: ImportSource,
    },

    /// Define a locally-defined resource: bind its destructor, if it
    /// has one, to the core function the earlier directives produced.
    /// The resource's [`ResourceSpec::Local`] entry names the source.
    DefineResource {
        /// Index into [`ExecutorIr::resources`].
        resource_index: usize,
    },

    /// Extract a core function and bind it to the next runtime-
    /// realloc slot. Used to resolve `cabi_realloc` references in
    /// canonical-ABI lowering.
    ExtractRealloc {
        /// The runtime-realloc slot this directive populates.
        slot: usize,
        /// Where the underlying core function comes from.
        source: ImportSource,
    },

    /// Extract a core function and bind it to the next runtime-
    /// post-return slot. Used to invoke `post-return` after a sync
    /// lift returns to the caller.
    ExtractPostReturn {
        /// The runtime-post-return slot this directive populates.
        slot: usize,
        /// Where the underlying core function comes from.
        source: ImportSource,
    },

    /// Extract a core function and bind it to the next runtime-
    /// callback slot. The callback of an export lifted `canon lift
    /// async (callback ...)` is resumed once per event the task
    /// receives, and the validator has already checked that its
    /// core type is `(func (param i32 i32 i32) (result i32))`.
    ExtractCallback {
        /// The runtime-callback slot this directive populates.
        slot: usize,
        /// Where the underlying core function comes from.
        source: ImportSource,
    },
}

/// One import an imported core module receives, by the two-level
/// name the module asks for it under.
pub struct NamedImportSource {
    /// The first-level name.
    pub module: String,
    /// The second-level name.
    pub name: String,
    /// Where the item comes from.
    pub source: ImportSource,
}

/// Where a single core-Wasm item comes from when satisfying a
/// module import or a component export.
pub enum ImportSource {
    /// An export of an already-instantiated core-Wasm instance.
    CoreInstanceExport(CoreInstanceExport),
    /// A host trampoline. The carried index is into
    /// [`ExecutorIr::trampoline_specs`].
    Trampoline(usize),
    /// The `may_leave` flags global of the component instance at
    /// the carried index. Adapter modules import it to clear the
    /// flag while they translate arguments and results across a
    /// component boundary, and to trap when a component that may
    /// not be left is called.
    InstanceFlags(usize),
}

/// An export taken from a previously-instantiated core-Wasm
/// instance.
pub struct CoreInstanceExport {
    /// Index of the runtime instance. The executor builds a list
    /// of runtime instances in the order [`Initializer::InstantiateModule`]
    /// directives appear; this index is into that list.
    pub instance_index: usize,
    /// The export item — either a name the runtime-layer instance
    /// can `get_export` directly, or an entity index the executor
    /// resolves through that instance's [`ModuleEntry::entity_to_name`]
    /// table.
    pub item: CoreSourceItem,
}

/// How an instance export is identified.
pub enum CoreSourceItem {
    /// The export name the underlying core module declares.
    Name(String),
    /// The export's entity-index inside the underlying core
    /// module. The executor turns this into a name through the
    /// owning module's [`ModuleEntry::entity_to_name`] table.
    ///
    /// The translator resolves names to indices for static modules
    /// to avoid runtime name lookups, so this is the common form.
    Index(EntityIndex),
}

/// One component-level function export the executor exposes to the
/// caller through [`crate::Instance::get_func`] or through the
/// [`crate::Instance::exports`] navigator.
pub struct ExportSpec {
    /// The leaf name the export is declared under. For root-level
    /// exports this is the name the component publishes; for
    /// instance-typed exports this is the item name inside the
    /// enclosing instance.
    pub name: String,
    /// The names of the instance-typed exports that enclose this
    /// function, from the root of the export tree inward, or empty
    /// for a root-level function export.
    pub path: Box<[ExternalName]>,
    /// Where the underlying core-Wasm function lives.
    pub source: ImportSource,
    /// The component-level signature the lift produced, with its
    /// canonical-ABI layout. Shared with every instance's handle for
    /// the export and with the task record of every call through it.
    pub signature: Arc<Signature>,
    /// The canonical-ABI options the lift declared, shared the same
    /// way.
    pub options: Arc<CanonOptions>,
}

/// Canonical-ABI options associated with a single lifted or lowered
/// function.
///
/// Indexes reference the runtime slabs the corresponding `Extract*`
/// initializer populates; `None` means the option was not declared
/// (e.g. a function whose ABI does not need `realloc`).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CanonOptions {
    /// The component instance the lift or lower belongs to, by the
    /// translator's per-instantiation index. Names the instance
    /// record whose entry gate, backpressure, and flags the call
    /// consults.
    pub instance: usize,
    /// Index into the `num_runtime_memories` slab on
    /// [`ExecutorIr`]. `None` when the function does not declare a
    /// memory option.
    pub memory: Option<usize>,
    /// Index into the `num_runtime_reallocs` slab on
    /// [`ExecutorIr`]. `None` when the function does not declare a
    /// realloc option.
    pub realloc: Option<usize>,
    /// Index into the `num_runtime_post_returns` slab on
    /// [`ExecutorIr`]. `None` when the function does not declare a
    /// post-return option.
    pub post_return: Option<usize>,
    /// Whether the lift or lower declared the `async` option. A
    /// lift that declares it returns a status word instead of the
    /// result, and names the callback below.
    pub async_: bool,
    /// Index into the `num_runtime_callbacks` slab on
    /// [`ExecutorIr`]. `None` when the function declares no
    /// callback, which for a lift that is `async_` is the stackful
    /// form the polyfill refuses at translation.
    pub callback: Option<usize>,
    /// The string encoding the lift or lower uses for
    /// `string`-typed values.
    pub string_encoding: StringEncoding,
    /// Where the values the lift or lower carries live. Linear
    /// memory is the only data model the polyfill implements, so a
    /// `canon` definition that declares another one is rejected at
    /// translation.
    pub data_model: DataModel,
}

/// Where the values a [`CanonOptions`] bundle governs live.
///
/// The canonical ABI has one strategy per data model. The
/// linear-memory model stores a value at a byte offset the caller
/// supplied or `cabi_realloc` returned, and loads a value from a
/// pointer. The garbage-collected model keeps the value in the
/// collected heap instead. The polyfill implements the first, and
/// translation rejects a component that declares the second.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DataModel {
    /// Values live in the component instance's linear memory.
    LinearMemory,
    /// Values live in the garbage-collected heap.
    Gc,
}

/// The string encoding a [`CanonOptions`] bundle declares.
///
/// The polyfill mirrors the three encodings the Component Model
/// recognises. Today only [`StringEncoding::Utf8`] is exercised by
/// the synchronous baseline tests; UTF-16 and Latin-1+UTF-16 are
/// preserved through the IR so downstream work can implement them
/// without reshaping the canon-options surface.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StringEncoding {
    /// UTF-8 encoding (the default).
    Utf8,
    /// UTF-16 encoding.
    Utf16,
    /// Latin-1 (ISO-8859-1) with UTF-16 fallback for code points
    /// above U+00FF.
    CompactUtf16,
}

/// Per-`LowerImport` metadata: where to draw the host registration
/// from, the canon options the lowering uses, which lowering the
/// `canon lower` declared, and the lifted (component-level) function
/// type the host is expected to satisfy.
///
/// The lowering's kind and the `async` effect on `signature` are
/// separate axes. The kind is the `async` option of the `canon
/// lower`: it decides the core signature the guest calls through and
/// what the guest gets back when the call does not finish at once.
/// The effect on the type is the callee's: it decides how the callee
/// produces its result, and it is what the `canon lift` at the other
/// end of the call reads. A guest may lower an async-typed import
/// either way, so both kinds reach this spec carrying the effect;
/// only an asynchronous lower requires it.
#[derive(Clone, Debug)]
pub struct LoweringSpec {
    /// Index into the resolved component imports — the same indexing
    /// `Component::imports` and `Resolution::bindings` use.
    pub import_index: usize,
    /// The names from the imported instance down to the function
    /// the lowering targets, one per nesting level. Empty when the
    /// import itself is the target (a plain-named function import).
    pub path: Box<[String]>,
    /// The host-side function type the registration must declare,
    /// with its canonical-ABI layout. Every trampoline built from the
    /// spec shares it, so a call never lays the parameters out again.
    pub signature: Arc<Signature>,
    /// The canon options the lower uses to translate between the
    /// host's `Val` shape and the core-wasm flat values the
    /// trampoline shuttles.
    pub options: Arc<CanonOptions>,
    /// Which lowering the `canon lower` declared, read from the
    /// `async` option of `options` at translation. The two kinds
    /// present different core signatures to the guest, so the
    /// trampoline is built from this and not from the effect the
    /// callee's type carries.
    pub kind: LowerKind,
}

/// What a trampoline slot in [`ExecutorIr::trampoline_specs`] does.
///
/// Each variant captures the polyfill-side metadata needed to build
/// the corresponding runtime-layer host function at instantiation
/// time. Lowered imports dispatch into a host registration; a
/// resource intrinsic names one of the translator's resource tables
/// by index, which instantiation maps to the one handle table the
/// owning component instance keeps.
#[derive(Clone, Debug)]
pub enum TrampolineSpec {
    /// The trampoline lowers a host import: lifts core arguments to
    /// [`Val`], dispatches into a host-function registration, and
    /// lowers the host's [`Val`] return back into core slots.
    ///
    /// [`Val`]: crate::Val
    LowerImport(LoweringSpec),
    /// The trampoline implements the canonical `resource.drop`
    /// intrinsic: removes the named resource handle from the owning
    /// instance's table and runs the host destructor with the
    /// entry's rep.
    ResourceDrop {
        /// Index into [`ExecutorIr::resource_tables`].
        table_index: usize,
    },
    /// The trampoline implements `resource.new`: allocates a fresh
    /// handle for the rep argument and returns the index.
    ResourceNew {
        /// Index into [`ExecutorIr::resource_tables`].
        table_index: usize,
    },
    /// The trampoline implements `resource.rep`: returns the rep of
    /// the handle at the given index without removing it.
    ResourceRep {
        /// Index into [`ExecutorIr::resource_tables`].
        table_index: usize,
    },
    /// A string transcoder an adapter module imports to move a
    /// string between two components' memories.
    Transcoder {
        /// Which conversion the transcoder performs.
        op: TranscodeOp,
        /// The runtime memory slot of the source memory.
        from_memory: usize,
        /// The runtime memory slot of the destination memory.
        to_memory: usize,
        /// The core signature the adapter imports.
        signature: CoreSignature,
    },
    /// An adapter transfers an `own<T>` handle from one component
    /// instance's table to another's.
    ResourceTransferOwn {
        /// The core signature the adapter imports.
        signature: CoreSignature,
    },
    /// An adapter lends a `borrow<T>` handle from one component
    /// instance's table to another's.
    ResourceTransferBorrow {
        /// The core signature the adapter imports.
        signature: CoreSignature,
    },
    /// An adapter transfers the readable end of a stream or a future
    /// from one component instance's table to another's. The
    /// `StreamTransfer` and `FutureTransfer` intrinsics take the same
    /// three arguments — the source index and the source and
    /// destination tables — so both are this one variant, over the
    /// tables of their own kind.
    EndTransfer {
        /// Every stream table of the component for a
        /// `StreamTransfer`, or every future table for a
        /// `FutureTransfer`, at the translator's table index.
        tables: Arc<[EndTableSpec]>,
        /// The core signature the adapter imports.
        signature: CoreSignature,
    },
    /// An adapter raises a trap with a Wasmtime trap code.
    Trap {
        /// The core signature the adapter imports.
        signature: CoreSignature,
        /// The trap code, as `wasmtime-environ` numbers them. The
        /// adapter imports one such intrinsic per code it can raise
        /// and calls it with no arguments.
        code: u8,
    },
    /// An adapter enters a synchronous call into another component
    /// instance.
    EnterSyncCall {
        /// The core signature the adapter imports.
        signature: CoreSignature,
    },
    /// An adapter leaves a synchronous call into another component
    /// instance.
    ExitSyncCall {
        /// The core signature the adapter imports.
        signature: CoreSignature,
    },
    /// An adapter reads one of the current task's context slots.
    ContextGet {
        /// Which slot, 0 or 1.
        slot: usize,
        /// The core signature the adapter imports.
        signature: CoreSignature,
    },
    /// An adapter writes one of the current task's context slots.
    ContextSet {
        /// Which slot, 0 or 1.
        slot: usize,
        /// The core signature the adapter imports.
        signature: CoreSignature,
    },
    /// The guest raises the backpressure of one component instance,
    /// which shuts that instance's entry gate.
    BackpressureInc {
        /// The component instance whose counter the built-in
        /// raises, by the translator's per-instantiation index.
        instance: usize,
        /// The core signature the guest imports.
        signature: CoreSignature,
    },
    /// The guest lowers the backpressure of one component instance.
    /// The gate opens again once the counter is back at zero.
    BackpressureDec {
        /// The component instance whose counter the built-in
        /// lowers, by the translator's per-instantiation index.
        instance: usize,
        /// The core signature the guest imports.
        signature: CoreSignature,
    },
    /// The trampoline implements `task.return`: it lifts the result
    /// the guest passes it and resolves the current task with it.
    TaskReturn {
        /// The result type the built-in was declared with, or `None`
        /// when it was declared with no result. The built-in traps
        /// unless it equals the result of the function the current
        /// task is a call into.
        result: Option<ValueType>,
        /// The interned index of that same result tuple. A task the
        /// prepare intrinsic created carries no projected function
        /// type, because the adapter names the type by index at run
        /// time, so the comparison for such a task is of indices.
        result_tuple: usize,
        /// The canon options the built-in was declared with: the
        /// lift of the result runs under them, and their string
        /// encoding and memory must equal the ones the task's own
        /// lift declared.
        options: CanonOptions,
        /// The core signature the guest imports: the flattened
        /// result as parameters, or one `i32` pointer when the
        /// flattened result exceeds sixteen values.
        signature: CoreSignature,
    },
    /// The `waitable-set.new` built-in: a waitable set record enters
    /// the store and its index in the calling instance's handle
    /// table is returned.
    WaitableSetNew {
        /// The component instance that calls the built-in, by the
        /// translator's per-instantiation index.
        instance: usize,
        /// The core signature the guest imports.
        signature: CoreSignature,
    },
    /// The `waitable-set.wait` built-in: the calling thread waits
    /// until the named set holds an event, and the event's payloads
    /// are written through the built-in's own memory.
    WaitableSetWait {
        /// The canon options the built-in declared, which name the
        /// component instance and the memory the payloads are
        /// written through.
        options: CanonOptions,
        /// The core signature the guest imports.
        signature: CoreSignature,
    },
    /// The `waitable-set.poll` built-in: as
    /// [`TrampolineSpec::WaitableSetWait`], but it never blocks.
    WaitableSetPoll {
        /// The canon options the built-in declared.
        options: CanonOptions,
        /// The core signature the guest imports.
        signature: CoreSignature,
    },
    /// The `waitable-set.drop` built-in: the named set's entry
    /// leaves the calling instance's handle table and its record
    /// leaves the store.
    WaitableSetDrop {
        /// The component instance that calls the built-in.
        instance: usize,
        /// The core signature the guest imports.
        signature: CoreSignature,
    },
    /// The `waitable.join` built-in: the named waitable joins the
    /// named set, or leaves the set it is in when the set index is
    /// zero.
    WaitableJoin {
        /// The component instance that calls the built-in.
        instance: usize,
        /// The core signature the guest imports.
        signature: CoreSignature,
    },
    /// The `subtask.drop` built-in: the named subtask's entry leaves
    /// the calling instance's handle table, and the records the
    /// entry named leave the store with it.
    SubtaskDrop {
        /// The component instance that calls the built-in.
        instance: usize,
        /// The core signature the guest imports.
        signature: CoreSignature,
    },
    /// The `stream.new` built-in: a stream's shared record and its
    /// two end records enter the store, and the built-in returns the
    /// readable end's index in the calling instance's handle table in
    /// the low half of an `i64` and the writable end's in the high
    /// half.
    StreamNew {
        /// The component instance that calls the built-in.
        instance: usize,
        /// The type of each value the stream carries, or `None` for a
        /// stream that carries none.
        payload: Option<ValueType>,
        /// The core signature the guest imports.
        signature: CoreSignature,
    },
    /// The `future.new` built-in: as [`TrampolineSpec::StreamNew`],
    /// for a future.
    FutureNew {
        /// The component instance that calls the built-in.
        instance: usize,
        /// The type of the value the future carries, or `None` for a
        /// future that carries none.
        payload: Option<ValueType>,
        /// The core signature the guest imports.
        signature: CoreSignature,
    },
    /// One of the four drop built-ins of a stream or future end,
    /// `stream.drop-readable`, `stream.drop-writable`,
    /// `future.drop-readable`, and `future.drop-writable`: the named
    /// end's entry leaves the calling instance's handle table, and
    /// the end is dropped.
    DropEnd {
        /// The kind of end the built-in drops.
        kind: EndKind,
        /// The component instance that calls the built-in.
        instance: usize,
        /// The payload type the built-in was declared with, which the
        /// end's stream or future must carry.
        payload: Option<ValueType>,
        /// The core signature the guest imports.
        signature: CoreSignature,
    },
    /// `stream.read`, `stream.write`, `future.read`, or
    /// `future.write`: a copy on the named end, into or out of a
    /// buffer in guest memory, which pairs with a copy on the other
    /// end of the stream or future.
    Copy {
        /// The kind of end the built-in copies on: the readable end
        /// for a read, the writable end for a write.
        kind: EndKind,
        /// The canon options the built-in was declared with: the
        /// calling instance, the `async` flag, and the memory the
        /// buffer lives in, with the `realloc` a value lowered into
        /// it may call.
        options: CanonOptions,
        /// The payload type the built-in was declared with, which the
        /// end's stream or future must carry.
        payload: Option<ValueType>,
        /// Whether the payload is a number type or absent, so that the
        /// copy moves bytes rather than values and a read and a write
        /// from one instance may meet.
        copies_bytes: bool,
        /// The core signature the guest imports.
        signature: CoreSignature,
    },
    /// `stream.cancel-read`, `stream.cancel-write`,
    /// `future.cancel-read`, or `future.cancel-write`: a cancel of the
    /// copy in progress on the named end, which returns the packed
    /// result that reports the copy.
    CancelCopy {
        /// The kind of end the built-in cancels a copy on: the
        /// readable end for a read, the writable end for a write.
        kind: EndKind,
        /// The component instance that calls the built-in.
        instance: usize,
        /// Whether the built-in was declared `async`, and so returns
        /// the blocked sentinel rather than wait when the cancel has
        /// not finished.
        async_: bool,
        /// The payload type the built-in was declared with, which the
        /// end's stream or future must carry.
        payload: Option<ValueType>,
        /// The core signature the guest imports: the end's index in
        /// and the packed result out.
        signature: CoreSignature,
    },
    /// The `prepare-call` intrinsic of a fused adapter whose lower
    /// or lift is asynchronous: it creates the callee's task and the
    /// caller's subtask, and records on the subtask the two
    /// functions the adapter generated for the call.
    PrepareCall {
        /// The runtime memory slot the callee's lift named, which
        /// the callee's `task.return` must name too. `None` when the
        /// lift named no memory.
        memory: Option<usize>,
        /// The core signature the adapter imports: the two
        /// `funcref`s, the six numbers the protocol carries, and
        /// then the caller's own flat arguments.
        signature: CoreSignature,
    },
    /// The `sync-start-call` intrinsic of a fused adapter whose
    /// lower is synchronous and whose lift is asynchronous: it runs
    /// the prepared call and blocks the caller until it resolves.
    SyncStartCall {
        /// The runtime callback slot of the callee's lift. The
        /// stackful form of `canon lift async` names none, and
        /// translation refuses it, so the slot is always filled here.
        callback: usize,
        /// The core signature the adapter imports: the callee's
        /// `funcref` and its flat parameter count in, and the
        /// caller's flat results out.
        signature: CoreSignature,
    },
    /// The `async-start-call` intrinsic of a fused adapter whose
    /// lower is asynchronous: it runs the prepared call and answers
    /// with the status word, so the caller gets control back before
    /// the callee necessarily returns.
    AsyncStartCall {
        /// The runtime callback slot of an asynchronously lifted
        /// callee. `None` for a synchronously lifted one, and for
        /// the stackful form, which the adapter distinguishes only
        /// by the flag word it passes at the call.
        callback: Option<usize>,
        /// The runtime post-return slot of a synchronously lifted
        /// callee, which runs once its results have crossed. `None`
        /// when the callee's lift declared none.
        post_return: Option<usize>,
        /// The core signature the adapter imports: the callee's
        /// `funcref`, its flat parameter and result counts, and the
        /// flag word in; the status word out.
        signature: CoreSignature,
    },
    /// The `thread.yield` built-in: the calling thread gives way to
    /// the work the store already holds, and the built-in returns
    /// zero.
    ThreadYield {
        /// The component instance that calls the built-in.
        instance: usize,
        /// The core signature the guest imports: no parameters and
        /// one `i32` result.
        signature: CoreSignature,
    },
    /// The `task.cancel` built-in. Cancellation is not built, so a
    /// call fails with [`Error::Unsupported`](crate::Error) once the
    /// may-leave check has passed. The built-in is accepted so that
    /// a guest whose binding layer links it runs every path that
    /// does not cancel.
    TaskCancel {
        /// The component instance that calls the built-in.
        instance: usize,
        /// The core signature the guest imports: no parameters and
        /// no results.
        signature: CoreSignature,
    },
    /// The `subtask.cancel` built-in, accepted and failing at the
    /// call as [`TrampolineSpec::TaskCancel`] does.
    SubtaskCancel {
        /// The component instance that calls the built-in.
        instance: usize,
        /// The core signature the guest imports: the subtask index
        /// in and the subtask's state out.
        signature: CoreSignature,
    },
}

/// The core-Wasm signature of an intrinsic an adapter module
/// imports, as the translator declares it.
#[derive(Clone, Debug)]
pub struct CoreSignature {
    /// The parameter types, in order.
    pub params: Vec<CoreParameter>,
    /// The result types, in order.
    pub results: Vec<FlatType>,
}

/// One parameter of an intrinsic's core signature.
///
/// Every intrinsic but the prepare-and-start pair takes flat
/// canonical-ABI values alone. The prepare intrinsic carries the two
/// functions the adapter generated for the call, and a start
/// intrinsic carries the callee's core function, each as a
/// `funcref`, so a parameter is one or the other. No intrinsic
/// returns a `funcref`, so a result is always a flat value.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CoreParameter {
    /// A flat canonical-ABI value.
    Value(FlatType),
    /// A function reference.
    FuncRef,
}

/// The conversion a [`TrampolineSpec::Transcoder`] performs. The
/// variants mirror the transcoders the fused adapter compiler emits;
/// their argument and result conventions are documented on
/// [`crate::executor::intrinsics`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TranscodeOp {
    /// Validate and copy a UTF-8 string.
    CopyUtf8,
    /// Validate and copy a UTF-16 string.
    CopyUtf16,
    /// Copy a Latin-1 string.
    CopyLatin1,
    /// Inflate Latin-1 bytes to UTF-16 code units.
    Latin1ToUtf16,
    /// Convert Latin-1 bytes to UTF-8, possibly partially.
    Latin1ToUtf8,
    /// Copy UTF-16, deflating to Latin-1 when every code point fits.
    Utf16ToCompactProbablyUtf16,
    /// Finish a Latin-1-then-UTF-16 conversion from UTF-16 input.
    Utf16ToCompactUtf16,
    /// Deflate UTF-16 to Latin-1 as far as the code points allow.
    Utf16ToLatin1,
    /// Convert UTF-16 to UTF-8, possibly partially.
    Utf16ToUtf8,
    /// Finish a Latin-1-then-UTF-16 conversion from UTF-8 input.
    Utf8ToCompactUtf16,
    /// Deflate UTF-8 to Latin-1 as far as the code points allow.
    Utf8ToLatin1,
    /// Convert UTF-8 to UTF-16.
    Utf8ToUtf16,
}

/// One resource table of the component: the resource it holds and
/// the component instance that keeps it.
#[derive(Clone, Debug)]
pub struct ResourceTableSpec {
    /// Index into [`ExecutorIr::resources`].
    pub resource_index: usize,
    /// The component instance (by runtime index) that keeps the table.
    pub instance: usize,
    /// Whether that instance is the one that defines the resource. A
    /// borrow lowered into the defining instance is the rep itself.
    pub defining: bool,
    /// The resource the table holds, as the component names it: the
    /// label the resource is imported or exported under, and the
    /// index of this table. This is the only label a resource the
    /// component defines has, so an error about a handle of such a
    /// resource renders it. A resource the component imports is
    /// named instead from the label the linker registered the host
    /// resource under, which is the same label the component
    /// imports it by, because that label is what the resolver
    /// matched the registration on.
    pub resource_type: ResourceType,
}

/// One stream or future table of the component: the type of the ends
/// it holds and the component instance that keeps them. The
/// translator gives each component instance one table per stream or
/// future type it uses, and a transfer intrinsic names a table by its
/// index to say both whose handle table the end is in and which type
/// it crosses as.
#[derive(Clone, Debug)]
pub struct EndTableSpec {
    /// The component instance (by runtime index) that keeps the
    /// table's ends in its handle table.
    pub instance: usize,
    /// The `stream<T>` or `future<T>` type of the table's ends.
    pub ty: ValueType,
}

/// Per-resource metadata captured during translation, indexed by the
/// translator's resource index: the component's imported resources
/// first, in import order, then the resources it defines.
pub enum ResourceSpec {
    /// A resource type the component imports. The executor resolves
    /// it at instantiation time against the [`Linker`]'s registered
    /// host resources.
    ///
    /// [`Linker`]: crate::Linker
    Imported {
        /// Index into the polyfill component's imports
        /// (`Component::imports`). Identifies the imported instance
        /// the resource lives in, or — when `item_name` is `None` —
        /// the import that is itself a resource type.
        import_index: usize,
        /// The names from the imported instance down to the resource,
        /// one per nesting level. Empty when the import is itself the
        /// resource type (a top-level resource import).
        path: Box<[String]>,
    },
    /// A resource type the component defines. Its identity is minted
    /// fresh at every instantiation, and its destructor, when it has
    /// one, is a core function of the defining component instance.
    Local {
        /// The component instance (by runtime index) that defines the
        /// resource.
        instance: usize,
        /// Where the destructor comes from, or `None` for a resource
        /// without one. Bound when the [`Initializer::DefineResource`]
        /// directive runs, after the defining core instance exists.
        destructor: Option<ImportSource>,
    },
}
