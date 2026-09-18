//! The executor's owned, target-agnostic IR.
//!
//! The polyfill's executor consumes a small, polyfill-owned shape
//! rather than `wasmtime_environ::component::Component` directly, so
//! the executor never names a translator type. One translator,
//! `wasmtime_environ`'s component `Translator`, runs on every target
//! and is projected into [`ExecutorIr`].

use std::collections::HashMap;

use crate::abi::layout::FlatType;
use crate::component::{ExternalName, FunctionType};
use crate::module::Module;
use crate::types::ValueType;

/// The executor's IR for a single parsed component.
///
/// All fields are owned and indexed in declaration order. The
/// executor walks `initializers` to drive substrate-level
/// instantiation, then walks `exports` to expose component-level
/// function handles.
///
/// The five `num_runtime_*` fields name the slab sizes the
/// initializers populate. Slot ordering matches the order in which
/// the corresponding `Extract*` / `LowerImport` initializer
/// produces its entry. `CanonOptions` and `ImportSource::Trampoline`
/// reference these slabs by index.
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
    /// The component-level signature the lift produced.
    pub signature: FunctionType,
    /// The canonical-ABI options the lift declared.
    pub options: CanonOptions,
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
/// from, the canon options the lowering uses, and the lifted
/// (component-level) function type the host is expected to satisfy.
#[derive(Clone, Debug)]
pub struct LoweringSpec {
    /// Index into the resolved component imports — the same indexing
    /// `Component::imports` and `Resolution::bindings` use.
    pub import_index: usize,
    /// The names from the imported instance down to the function
    /// the lowering targets, one per nesting level. Empty when the
    /// import itself is the target (a plain-named function import).
    pub path: Box<[String]>,
    /// The host-side function type the registration must declare.
    pub signature: FunctionType,
    /// The canon options the lower uses to translate between the
    /// host's `Val` shape and the core-wasm flat values the
    /// trampoline shuttles.
    pub options: CanonOptions,
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
}

/// The core-Wasm signature of an intrinsic an adapter module
/// imports, as the translator declares it.
#[derive(Clone, Debug)]
pub struct CoreSignature {
    /// The parameter types, in order.
    pub params: Vec<FlatType>,
    /// The result types, in order.
    pub results: Vec<FlatType>,
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
    /// Whether that instance is the one that defines the resource. The
    /// defining instance handles reps directly for borrows.
    pub defining: bool,
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
