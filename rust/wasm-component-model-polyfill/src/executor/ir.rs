//! The executor's owned, target-agnostic IR.
//!
//! The polyfill's executor consumes a small, polyfill-owned shape
//! rather than `wasmtime_environ::component::Component` directly, so
//! the executor never names a translator type. One translator,
//! `wasmtime_environ`'s component `Translator`, runs on every target
//! and is projected into [`ExecutorIr`].

use std::collections::HashMap;

use crate::component::FunctionType;
use crate::identifier::InterfaceIdentifier;

/// The executor's IR for a single parsed component.
///
/// All fields are owned and indexed in declaration order. The
/// executor walks `initializers` to drive substrate-level
/// instantiation, then walks `exports` to expose component-level
/// function handles.
///
/// The four `num_runtime_*` fields name the slab sizes the
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
    /// The component's function exports, in declaration order.
    pub exports: Box<[ExportSpec]>,
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
    /// Maps each runtime-instance position (the index a
    /// [`CoreInstanceExport`] uses) to the polyfill's `modules`
    /// slot the instance was instantiated against. The runtime-
    /// instance position is the count of preceding
    /// [`Initializer::InstantiateModule`] directives, so the n-th
    /// entry here is the owning module of the n-th instance the
    /// executor builds.
    pub runtime_instance_to_module: Box<[usize]>,
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
}

/// One core module pre-translated to the runtime layer.
///
/// Carries the runtime-layer handle the executor instantiates
/// against alongside the module's import declarations (so the
/// executor can pair them with each instantiation's
/// [`ImportSource`] list) and an inverted export table (so an
/// `EntityIndex`-keyed lookup produces a name the runtime-layer
/// instance's `get_export` accepts).
pub struct ModuleEntry {
    /// The runtime-layer Module. Constructed at translation time
    /// from the core module's binary slice.
    pub runtime: wasm_runtime_layer::Module,
    /// The module's declared imports as `(host_namespace, name)`,
    /// in declaration order, read from
    /// `wasmtime_environ::Module::imports`.
    pub imports: Box<[ModuleImport]>,
    /// Inverted export table: maps each export's slot
    /// (entity-index) to the name the module declares it under.
    /// Used when a component's [`CoreSourceItem::Index`] resolves
    /// against this module.
    pub entity_to_name: HashMap<EntityIndex, String>,
}

/// One declared import of a core module (`(import "host" "name" ...)`).
pub struct ModuleImport {
    /// The host (first-level) name.
    pub host: String,
    /// The item (second-level) name.
    pub name: String,
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
}

/// Where a single core-Wasm item comes from when satisfying a
/// module import or a component export.
pub enum ImportSource {
    /// An export of an already-instantiated core-Wasm instance.
    CoreInstanceExport(CoreInstanceExport),
    /// A host trampoline produced by an
    /// [`Initializer::LowerImport`] directive. The carried index is
    /// into [`ExecutorIr::lowerings`].
    Trampoline(usize),
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
    /// The enclosing instance-typed export's identifier when this
    /// function is nested inside one, or `None` for a root-level
    /// function export.
    pub parent: Option<InterfaceIdentifier>,
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
#[derive(Clone, Debug)]
pub struct CanonOptions {
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
    /// The string encoding the lift or lower uses for
    /// `string`-typed values.
    pub string_encoding: StringEncoding,
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
    /// For interface-typed imports, the item name within the
    /// imported instance the lowered function targets. `None` when
    /// the import itself is the target (a plain-named function
    /// import).
    pub item_name: Option<String>,
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
/// time. Lowered imports dispatch into a host registration; resource
/// intrinsics dispatch into the per-store handle table for a named
/// resource type.
#[derive(Clone, Debug)]
pub enum TrampolineSpec {
    /// The trampoline lowers a host import: lifts core arguments to
    /// [`Val`], dispatches into a host-function registration, and
    /// lowers the host's [`Val`] return back into core slots.
    ///
    /// [`Val`]: crate::Val
    LowerImport(LoweringSpec),
    /// The trampoline implements the canonical `resource.drop`
    /// intrinsic: removes the named resource handle from the per-
    /// store table and runs the host destructor with the entry's
    /// rep.
    ResourceDrop {
        /// Index into [`ExecutorIr::resources`].
        resource_index: usize,
    },
    /// The trampoline implements `resource.new`: allocates a fresh
    /// handle for the rep argument and returns the index.
    ResourceNew {
        /// Index into [`ExecutorIr::resources`].
        resource_index: usize,
    },
    /// The trampoline implements `resource.rep`: returns the rep of
    /// the handle at the given index without removing it.
    ResourceRep {
        /// Index into [`ExecutorIr::resources`].
        resource_index: usize,
    },
}

/// Per-resource metadata captured during translation.
///
/// Identifies a host-imported resource type by the polyfill import
/// index of the enclosing imported instance and the resource's
/// label within that instance. The executor resolves this at
/// instantiation time against the [`Linker`]'s registered host
/// resources.
///
/// [`Linker`]: crate::Linker
#[derive(Clone, Debug)]
pub struct ResourceSpec {
    /// Index into the polyfill component's imports
    /// (`Component::imports`). Identifies the imported instance the
    /// resource lives in, or — when `item_name` is `None` — the
    /// import that is itself a resource type.
    pub import_index: usize,
    /// The resource's label within the imported instance. `None`
    /// when the import is itself the resource type (top-level
    /// resource import).
    pub item_name: Option<String>,
}
