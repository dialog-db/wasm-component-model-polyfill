//! The executor's owned, target-agnostic IR.
//!
//! The polyfill's executor consumes a small, polyfill-owned shape
//! rather than `wasmtime_environ::component::Component` directly, so
//! the same execution code drives every target. The native
//! translator projects from `wasmtime_environ`'s rich
//! `ComponentTranslation` into [`ExecutorIr`]; the web translator
//! walks `wasmparser` payloads to build the same shape.

use std::collections::HashMap;

/// The executor's IR for a single parsed component.
///
/// All three fields are owned and indexed in declaration order. The
/// executor walks `initializers` to drive substrate-level
/// instantiation, then walks `exports` to expose component-level
/// function handles.
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
    /// in declaration order. Native translation reads these from
    /// `wasmtime_environ::Module::imports`; web translation reads
    /// them by walking the inner `wasmparser` payloads of the core
    /// module section.
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
/// `wasmtime_environ::EntityIndex` so the native translator can
/// project from it without a name-mapping table; the web translator
/// produces these directly while walking the module's own export
/// section.
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
}

/// Where a single core-Wasm item comes from when satisfying a
/// module import or a component export.
pub enum ImportSource {
    /// An export of an already-instantiated core-Wasm instance.
    CoreInstanceExport(CoreInstanceExport),
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
    /// Only the native translator emits this variant (Wasmtime's
    /// `Translator` resolves names to indices for static modules
    /// to avoid runtime name lookups). The web translator only
    /// emits [`CoreSourceItem::Name`] entries.
    // Remove the `dead_code` allow once a target other than native
    // (or a native code path other than the projection from
    // `wasmtime_environ`) constructs this variant.
    #[allow(dead_code)]
    Index(EntityIndex),
}

/// One component-level function export the executor exposes to the
/// caller through [`crate::Instance::get_func`].
pub struct ExportSpec {
    /// The name the component declares the export under.
    pub name: String,
    /// Where the underlying core-Wasm function lives.
    pub source: ImportSource,
}
