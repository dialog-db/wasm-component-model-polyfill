//! Component translator.
//!
//! Drives [`wasmtime_environ::component::Translator`] over a
//! component binary, then projects the resulting rich IR into the
//! polyfill's own [`ExecutorIr`] shape so the executor's driver is
//! target-agnostic. The same translator runs on every supported
//! target — see [`super`] for why the `compile` feature builds
//! cleanly on `wasm32-unknown-unknown`.

use core::fmt::Display;
use std::collections::HashMap;

use wasm_runtime_layer::Module as RuntimeModule;
use wasmtime_environ::component::{
    CanonicalOptions as EnvironCanonOptions, ComponentTranslation, ComponentTypes,
    ComponentTypesBuilder, CoreDef, CoreExport, Export as ComponentExport,
    ExportItem as ComponentExportItem, ExtractMemory, ExtractPostReturn, ExtractRealloc,
    GlobalInitializer, InstantiateModule, LoweredIndex, ResourceIndex, RuntimeImportIndex,
    StaticModuleIndex, StringEncoding as EnvironStringEncoding, Trampoline, TrampolineIndex,
    Translator, TypeResourceTable, TypeResourceTableIndex,
};
use wasmtime_environ::wasmparser::{Validator, WasmFeatures};
use wasmtime_environ::{EntityIndex as EnvironEntityIndex, ScopeVec, Tunables};

use crate::component::{Component, ExternType, ExternalName, FunctionType, InstanceItem};
use crate::engine::Engine;
use crate::error::{Error, Result};
use crate::identifier::InterfaceIdentifier;

use super::ir::{
    CanonOptions, CoreInstanceExport, CoreSourceItem, EntityIndex, ExecutorIr, ExportSpec,
    ImportSource, Initializer, LoweringSpec, ModuleEntry, ModuleImport, ResourceSpec,
    StringEncoding, TrampolineSpec,
};

/// Translate `component`'s bytes against `engine` and return the
/// executor's IR.
pub fn translate(engine: &Engine, component: &Component) -> Result<ExecutorIr> {
    let bytes: &[u8] = &component.bytes;
    let scope = ScopeVec::new();
    let tunables = Tunables::default_u32();
    let mut validator = Validator::new_with_features(WasmFeatures::all());
    let mut types = ComponentTypesBuilder::new(&validator);

    let (translation, modules) = Translator::new(&tunables, &mut validator, &mut types, &scope)
        .translate(bytes)
        .map_err(translation_error)?;

    // The component types map is needed to resolve
    // `TypeResourceTableIndex` → `ResourceIndex` for the trampoline
    // pre-walk. `ComponentTypesBuilder::finish` consumes the
    // builder, which is fine — translation is finished.
    let (component_types, _) = types.finish(&translation.component);

    let mut module_entries: Vec<ModuleEntry> = Vec::with_capacity(modules.len());
    let mut module_index_for_static: HashMap<StaticModuleIndex, usize> =
        HashMap::with_capacity(modules.len());
    for (static_idx, module) in modules {
        let runtime = RuntimeModule::new(engine.inner(), module.wasm).map_err(translation_error)?;
        let imports = module
            .module
            .imports()
            .map(|(host, name, _)| ModuleImport {
                host: host.to_owned(),
                name: name.to_owned(),
            })
            .collect::<Vec<_>>()
            .into_boxed_slice();
        let entity_to_name = module
            .module
            .exports
            .iter()
            .map(|(atom, idx)| {
                (
                    lift_entity_index(*idx),
                    module.module.strings[*atom].to_owned(),
                )
            })
            .collect();
        module_index_for_static.insert(static_idx, module_entries.len());
        module_entries.push(ModuleEntry {
            runtime,
            imports,
            entity_to_name,
        });
    }

    // Pre-walk #1: build `LoweredIndex → RuntimeImportIndex` from
    // the initializer list. The trampoline pre-walk consults this
    // map to pair its `lower_ty`/`options` with the right import.
    let mut lowered_to_import: HashMap<LoweredIndex, RuntimeImportIndex> = HashMap::new();
    for initializer in &translation.component.initializers {
        if let GlobalInitializer::LowerImport { index, import } = initializer {
            lowered_to_import.insert(*index, *import);
        }
    }

    // Pre-walk #2: project every imported resource into a
    // `ResourceSpec`. Defined (locally-declared) resources are not
    // exercised by the synchronous baseline tests this PDD un-stubs;
    // see the `GlobalInitializer::Resource` arm below for the
    // deferral.
    let mut resources: Vec<ResourceSpec> = Vec::new();
    let mut resource_to_spec: HashMap<ResourceIndex, usize> = HashMap::new();
    for (resource_idx, runtime_import) in translation.component.imported_resources.iter() {
        let spec = build_resource_spec(component, &translation, *runtime_import)?;
        let slot = resources.len();
        resources.push(spec);
        resource_to_spec.insert(resource_idx, slot);
    }

    // Pre-walk #3: build a [`TrampolineSpec`] for every entry in
    // `translation.trampolines` whose kind the synchronous baseline
    // exercises. The resulting indices are consulted both by the
    // `Initializer::LowerImport` projection (via `LoweredIndex →
    // slot`) and by `CoreDef::Trampoline` resolution (via
    // `TrampolineIndex → slot`).
    let mut trampoline_specs: Vec<TrampolineSpec> = Vec::new();
    let mut lowered_to_spec: HashMap<LoweredIndex, usize> = HashMap::new();
    let mut trampoline_to_spec: HashMap<TrampolineIndex, usize> = HashMap::new();
    for (trampoline_idx, trampoline) in translation.trampolines.iter() {
        match trampoline {
            Trampoline::LowerImport {
                index: lowered_idx,
                options,
                ..
            } => {
                let runtime_import = *lowered_to_import.get(lowered_idx).ok_or_else(|| {
                    internal(
                        "Trampoline::LowerImport has no matching GlobalInitializer::LowerImport",
                    )
                })?;
                let canon = translation
                    .component
                    .options
                    .get(*options)
                    .ok_or_else(|| internal("Trampoline OptionsIndex out of bounds"))?;
                let lowering = build_lowering_spec(
                    component,
                    &translation,
                    runtime_import,
                    lift_canon_options(canon),
                )?;
                let slot = trampoline_specs.len();
                trampoline_specs.push(TrampolineSpec::LowerImport(lowering));
                lowered_to_spec.insert(*lowered_idx, slot);
                trampoline_to_spec.insert(trampoline_idx, slot);
            }
            Trampoline::ResourceDrop { ty, .. } => {
                let resource_index = resolve_resource_index(
                    &component_types,
                    &translation.component.imported_resources,
                    &resource_to_spec,
                    *ty,
                )?;
                let slot = trampoline_specs.len();
                trampoline_specs.push(TrampolineSpec::ResourceDrop { resource_index });
                trampoline_to_spec.insert(trampoline_idx, slot);
            }
            Trampoline::ResourceNew { ty, .. } => {
                let resource_index = resolve_resource_index(
                    &component_types,
                    &translation.component.imported_resources,
                    &resource_to_spec,
                    *ty,
                )?;
                let slot = trampoline_specs.len();
                trampoline_specs.push(TrampolineSpec::ResourceNew { resource_index });
                trampoline_to_spec.insert(trampoline_idx, slot);
            }
            Trampoline::ResourceRep { ty, .. } => {
                let resource_index = resolve_resource_index(
                    &component_types,
                    &translation.component.imported_resources,
                    &resource_to_spec,
                    *ty,
                )?;
                let slot = trampoline_specs.len();
                trampoline_specs.push(TrampolineSpec::ResourceRep { resource_index });
                trampoline_to_spec.insert(trampoline_idx, slot);
            }
            // Other trampoline kinds — string transcoders, async
            // intrinsics, futures, streams, etc. — fall outside the
            // synchronous baseline. `CoreDef::Trampoline` references
            // to one of these surface as the existing
            // `internal("CoreDef::Trampoline references ...")`
            // diagnostic at lift time.
            _ => {}
        }
    }

    // Main walk.
    let mut state = ProjectionState::new();

    for initializer in &translation.component.initializers {
        match initializer {
            GlobalInitializer::InstantiateModule(
                InstantiateModule::Static(static_idx, defs),
                _,
            ) => {
                let module_index = *module_index_for_static.get(static_idx).ok_or_else(|| {
                    internal("module index from translator missing from projection map")
                })?;
                let mut imports = Vec::with_capacity(defs.len());
                for def in defs.iter() {
                    imports.push(state.lift_core_def(def, &trampoline_to_spec)?);
                }
                state.runtime_instance_to_module.push(module_index);
                state.initializers.push(Initializer::InstantiateModule {
                    module_index,
                    imports: imports.into_boxed_slice(),
                });
            }
            GlobalInitializer::InstantiateModule(InstantiateModule::Import(_, _), _) => {
                todo!(
                    "import-style core-module instantiation: only Wasmtime's adapter pipeline emits this; not exercised by the synchronous baseline tests yet"
                )
            }
            GlobalInitializer::LowerImport { .. } => {
                // The trampoline's runtime-layer function is built
                // upfront from `ir.trampoline_specs` before the
                // initializer walk runs (see `executor::instantiate`).
                // The marker initializer adds no further state.
            }
            GlobalInitializer::ExtractMemory(ExtractMemory { index, export }) => {
                let source = state.lift_core_export(export)?;
                let slot = index.as_u32() as usize;
                state.num_runtime_memories = state.num_runtime_memories.max(slot + 1);
                state
                    .initializers
                    .push(Initializer::ExtractMemory { slot, source });
            }
            GlobalInitializer::ExtractRealloc(ExtractRealloc { index, def }) => {
                let source = state.lift_core_def(def, &trampoline_to_spec)?;
                let slot = index.as_u32() as usize;
                state.num_runtime_reallocs = state.num_runtime_reallocs.max(slot + 1);
                state
                    .initializers
                    .push(Initializer::ExtractRealloc { slot, source });
            }
            GlobalInitializer::ExtractPostReturn(ExtractPostReturn { index, def }) => {
                let source = state.lift_core_def(def, &trampoline_to_spec)?;
                let slot = index.as_u32() as usize;
                state.num_runtime_post_returns = state.num_runtime_post_returns.max(slot + 1);
                state
                    .initializers
                    .push(Initializer::ExtractPostReturn { slot, source });
            }
            GlobalInitializer::ExtractCallback(_) | GlobalInitializer::ExtractTable(_) => {
                todo!(
                    "callback / table extraction is async-tier or thread.spawn-indirect machinery — out of the synchronous baseline"
                )
            }
            GlobalInitializer::Resource(_) => {
                todo!(
                    "locally-defined resources (with an in-binary destructor) are not exercised by the synchronous baseline tests this PDD un-stubs; only host-imported resources are covered"
                )
            }
        }
    }

    let mut exports: Vec<ExportSpec> = Vec::new();
    for (name, (export_index, _)) in translation.component.exports.raw_iter() {
        collect_export(
            &translation,
            component,
            &mut state,
            &trampoline_to_spec,
            &mut exports,
            name,
            *export_index,
            None,
            None,
        )?;
    }

    Ok(ExecutorIr {
        modules: module_entries.into_boxed_slice(),
        initializers: state.initializers.into_boxed_slice(),
        exports: exports.into_boxed_slice(),
        trampoline_specs: trampoline_specs.into_boxed_slice(),
        resources: resources.into_boxed_slice(),
        runtime_instance_to_module: state.runtime_instance_to_module.into_boxed_slice(),
        num_runtime_memories: state.num_runtime_memories,
        num_runtime_reallocs: state.num_runtime_reallocs,
        num_runtime_post_returns: state.num_runtime_post_returns,
    })
}

/// Recursively project one component-level export into [`ExportSpec`]
/// entries. Root-level functions land as a single entry with no
/// parent; instance-typed exports recurse into their inner exports
/// with the enclosing instance's [`InterfaceIdentifier`] threaded
/// through as the `parent`. The `parent_path` carries the inner
/// item-name path for nested instances so the per-leaf signature
/// lookup can resolve into the polyfill's parsed-component view.
#[allow(clippy::too_many_arguments)]
fn collect_export(
    translation: &ComponentTranslation,
    component: &Component,
    state: &mut ProjectionState,
    trampoline_to_spec: &HashMap<TrampolineIndex, usize>,
    out: &mut Vec<ExportSpec>,
    name: &str,
    export_index: wasmtime_environ::component::ExportIndex,
    parent: Option<&InterfaceIdentifier>,
    parent_path: Option<&str>,
) -> Result<()> {
    let export = translation
        .component
        .export_items
        .get(export_index)
        .ok_or_else(|| internal("export index from translator missing from export_items"))?;
    match export {
        ComponentExport::LiftedFunction { func, options, .. } => {
            let source = state.lift_core_def(func, trampoline_to_spec)?;
            let signature = lookup_leaf_signature(component, parent, parent_path, name)?;
            let canon = translation
                .component
                .options
                .get(*options)
                .ok_or_else(|| internal("export OptionsIndex out of bounds"))?;
            out.push(ExportSpec {
                name: name.to_owned(),
                parent: parent.cloned(),
                source,
                signature,
                options: lift_canon_options(canon),
            });
            Ok(())
        }
        ComponentExport::ModuleStatic { .. } | ComponentExport::ModuleImport { .. } => {
            todo!(
                "module-typed component exports are out of scope for the synchronous baseline; see PDD003 checklist"
            )
        }
        ComponentExport::Instance { exports, .. } => {
            if parent.is_some() {
                todo!(
                    "doubly-nested instance exports — only one level of nesting is exercised by the synchronous baseline"
                );
            }
            let identifier = name.parse::<InterfaceIdentifier>().map_err(|err| {
                internal_msg(format!(
                    "instance-typed export name `{name}` does not parse as a WIT interface identifier: {err}"
                ))
            })?;
            for (item_name, (inner_index, _)) in exports.raw_iter() {
                collect_export(
                    translation,
                    component,
                    state,
                    trampoline_to_spec,
                    out,
                    item_name,
                    *inner_index,
                    Some(&identifier),
                    Some(name),
                )?;
            }
            Ok(())
        }
        ComponentExport::Type(_) => {
            // Type exports carry no runtime presence.
            Ok(())
        }
    }
}

/// Resolve a [`TypeResourceTableIndex`] to the polyfill's resource
/// index. The trampoline references resources by their per-component
/// table identity; the polyfill addresses them by position in the
/// `imported_resources` map.
fn resolve_resource_index(
    component_types: &ComponentTypes,
    imported_resources: &wasmtime_environ::PrimaryMap<ResourceIndex, RuntimeImportIndex>,
    resource_to_spec: &HashMap<ResourceIndex, usize>,
    ty: TypeResourceTableIndex,
) -> Result<usize> {
    let table = &component_types[ty];
    let resource_idx = match table {
        TypeResourceTable::Concrete { ty, .. } => *ty,
        TypeResourceTable::Abstract(_) => {
            return Err(internal(
                "resource trampoline references an abstract resource table — should not appear in a concrete instantiation",
            ));
        }
    };
    let _ = imported_resources; // arg kept for symmetry; lookup is via resource_to_spec.
    resource_to_spec.get(&resource_idx).copied().ok_or_else(|| {
        internal(
            "resource trampoline references a defined (non-imported) resource — locally-defined resources are not yet supported",
        )
    })
}

/// Build a [`ResourceSpec`] from a [`RuntimeImportIndex`] into the
/// component's import table. Mirrors [`build_lowering_spec`] but
/// resolves to a resource label rather than to a function signature.
fn build_resource_spec(
    component: &Component,
    translation: &ComponentTranslation,
    runtime_import: RuntimeImportIndex,
) -> Result<ResourceSpec> {
    let (import_idx, path) = translation
        .component
        .imports
        .get(runtime_import)
        .ok_or_else(|| internal("RuntimeImportIndex out of bounds"))?;
    let (top_name, _) = translation
        .component
        .import_types
        .get(*import_idx)
        .ok_or_else(|| internal("ImportIndex out of bounds for import_types"))?;

    let (polyfill_idx, _polyfill_import) = component
        .imports
        .iter()
        .enumerate()
        .find(|(_, imp)| matches_top_level_name(&imp.name, top_name))
        .ok_or_else(|| {
            internal("wasmtime resource import has no matching polyfill Component import by name")
        })?;

    let item_name = match path.len() {
        0 => None,
        1 => Some(path[0].clone()),
        _ => {
            todo!(
                "deeply-nested resource imports (path > 1) are not exercised by the synchronous baseline; the polyfill defers them until a test demands the shape"
            )
        }
    };

    Ok(ResourceSpec {
        import_index: polyfill_idx,
        item_name,
    })
}

/// Working state for the main initializer walk. Carries only the
/// data the main walk produces; trampoline/lowering tables are
/// pre-built and read-only at this stage.
struct ProjectionState {
    runtime_instance_to_module: Vec<usize>,
    initializers: Vec<Initializer>,
    num_runtime_memories: usize,
    num_runtime_reallocs: usize,
    num_runtime_post_returns: usize,
}

impl ProjectionState {
    fn new() -> Self {
        Self {
            runtime_instance_to_module: Vec::new(),
            initializers: Vec::new(),
            num_runtime_memories: 0,
            num_runtime_reallocs: 0,
            num_runtime_post_returns: 0,
        }
    }

    fn lift_core_def(
        &self,
        def: &CoreDef,
        trampoline_to_spec: &HashMap<TrampolineIndex, usize>,
    ) -> Result<ImportSource> {
        match def {
            CoreDef::Export(export) => self.lift_core_export(export),
            CoreDef::Trampoline(trampoline_idx) => {
                let lowering_index = *trampoline_to_spec.get(trampoline_idx).ok_or_else(|| {
                    internal(
                        "CoreDef::Trampoline references a trampoline kind the polyfill defers (resource intrinsics, transcoders, async)",
                    )
                })?;
                Ok(ImportSource::Trampoline(lowering_index))
            }
            CoreDef::InstanceFlags(_) => todo!(
                "component-instance flag globals are part of the canonical-ABI runtime state for asynchronous lifts; the synchronous baseline does not exercise them"
            ),
            CoreDef::UnsafeIntrinsic(_) => todo!(
                "Wasmtime unsafe intrinsics are not part of the polyfill's surface — see PDD003"
            ),
            CoreDef::TaskMayBlock => {
                todo!("task-may-block global is part of the async-tier runtime state — see PDD003")
            }
        }
    }

    fn lift_core_export<U: Copy + Into<EnvironEntityIndex>>(
        &self,
        export: &CoreExport<U>,
    ) -> Result<ImportSource> {
        let _ = self
            .runtime_instance_to_module
            .get(export.instance.as_u32() as usize)
            .ok_or_else(|| internal("CoreExport instance index missing from projection map"))?;
        let item = match &export.item {
            ComponentExportItem::Name(s) => CoreSourceItem::Name(s.clone()),
            ComponentExportItem::Index(idx) => {
                CoreSourceItem::Index(lift_entity_index((*idx).into()))
            }
        };
        Ok(ImportSource::CoreInstanceExport(CoreInstanceExport {
            instance_index: export.instance.as_u32() as usize,
            item,
        }))
    }
}

/// Resolve a `RuntimeImportIndex` against the polyfill's
/// [`Component`] and the wasmtime translation, then build a
/// [`LoweringSpec`] that the host-trampoline builder will consult at
/// instantiation time.
fn build_lowering_spec(
    component: &Component,
    translation: &ComponentTranslation,
    runtime_import: RuntimeImportIndex,
    options: CanonOptions,
) -> Result<LoweringSpec> {
    let (import_idx, path) = translation
        .component
        .imports
        .get(runtime_import)
        .ok_or_else(|| internal("RuntimeImportIndex out of bounds"))?;
    let (top_name, _) = translation
        .component
        .import_types
        .get(*import_idx)
        .ok_or_else(|| internal("ImportIndex out of bounds for import_types"))?;

    let (polyfill_idx, polyfill_import) = component
        .imports
        .iter()
        .enumerate()
        .find(|(_, imp)| matches_top_level_name(&imp.name, top_name))
        .ok_or_else(|| {
            internal("wasmtime import has no matching polyfill Component import by name")
        })?;

    // Walk the path of inner-instance lookups to find the leaf item.
    // The synchronous baseline's lowered imports are always
    // functions, so the final type must be Function.
    let (item_name, signature) = if path.is_empty() {
        let signature = match &polyfill_import.ty {
            ExternType::Function(ty) => ty.clone(),
            other => {
                return Err(internal_msg(format!(
                    "plain-named import is not a function: {other:?}"
                )));
            }
        };
        (None, signature)
    } else if path.len() == 1 {
        let leaf = &path[0];
        let signature = match &polyfill_import.ty {
            ExternType::Instance(instance) => {
                let item = instance
                    .items
                    .iter()
                    .find(|InstanceItem { name, .. }| name == leaf)
                    .ok_or_else(|| {
                        internal_msg(format!("imported instance has no item named `{leaf}`"))
                    })?;
                match &item.ty {
                    ExternType::Function(ty) => ty.clone(),
                    other => {
                        return Err(internal_msg(format!(
                            "imported instance item `{leaf}` is not a function: {other:?}"
                        )));
                    }
                }
            }
            other => {
                return Err(internal_msg(format!(
                    "lowered import expects an interface-typed instance, found {other:?}"
                )));
            }
        };
        (Some(leaf.clone()), signature)
    } else {
        todo!(
            "deeply-nested instance imports (path > 1) are not exercised by the synchronous baseline; the polyfill defers them until a test demands the shape"
        )
    };

    Ok(LoweringSpec {
        import_index: polyfill_idx,
        item_name,
        signature,
        options,
    })
}

/// Compare the polyfill's [`ExternalName`] against the wasmtime
/// import-types entry's wire-name (`String`). The wire form is
/// `"namespace:package/interface@version"` for interface-named
/// imports, the bare item name for plain-named imports.
fn matches_top_level_name(polyfill: &ExternalName, wire: &str) -> bool {
    match polyfill {
        ExternalName::Plain(name) => name == wire,
        ExternalName::Interface(id) => id.to_string() == wire,
    }
}

/// Resolve the polyfill [`FunctionType`] for one lifted-function
/// export. When `parent_path` is `Some`, look up the parent
/// instance-typed export first and find `leaf` inside its declared
/// items; otherwise the export is at the root of the component and
/// `leaf` is the wire-name the binary publishes.
fn lookup_leaf_signature(
    component: &Component,
    parent: Option<&InterfaceIdentifier>,
    parent_path: Option<&str>,
    leaf: &str,
) -> Result<FunctionType> {
    match (parent, parent_path) {
        (Some(_), Some(parent_wire)) => {
            for export in component.exports.iter() {
                let matches = match &export.name {
                    ExternalName::Plain(text) => text == parent_wire,
                    ExternalName::Interface(id) => id.to_string() == parent_wire,
                };
                if matches {
                    let instance = match &export.ty {
                        ExternType::Instance(instance) => instance,
                        _ => {
                            return Err(internal_msg(format!(
                                "export `{parent_wire}` is not an instance in the polyfill's parsed-component view"
                            )));
                        }
                    };
                    let item = instance
                        .items
                        .iter()
                        .find(|InstanceItem { name, .. }| name == leaf)
                        .ok_or_else(|| {
                            internal_msg(format!(
                                "instance export `{parent_wire}` has no item named `{leaf}`"
                            ))
                        })?;
                    return match &item.ty {
                        ExternType::Function(ty) => Ok(ty.clone()),
                        _ => Err(internal_msg(format!(
                            "instance export `{parent_wire}` item `{leaf}` is not a function"
                        ))),
                    };
                }
            }
            Err(internal_msg(format!(
                "instance export `{parent_wire}` not present in the polyfill's parsed-component view"
            )))
        }
        _ => {
            for export in component.exports.iter() {
                let matches = match &export.name {
                    ExternalName::Plain(text) => text == leaf,
                    ExternalName::Interface(id) => id.to_string() == leaf,
                };
                if matches {
                    return match &export.ty {
                        ExternType::Function(ty) => Ok(ty.clone()),
                        _ => Err(internal(
                            "lifted-function export's polyfill type is not a function",
                        )),
                    };
                }
            }
            Err(internal(
                "lifted-function export not present in the polyfill's parsed-component view",
            ))
        }
    }
}

fn lift_canon_options(options: &EnvironCanonOptions) -> CanonOptions {
    CanonOptions {
        memory: options.memory().map(|i| i.as_u32() as usize),
        realloc: match options.data_model {
            wasmtime_environ::component::CanonicalOptionsDataModel::LinearMemory(opts) => {
                opts.realloc.map(|i| i.as_u32() as usize)
            }
            wasmtime_environ::component::CanonicalOptionsDataModel::Gc {} => None,
        },
        post_return: options.post_return.map(|i| i.as_u32() as usize),
        string_encoding: lift_string_encoding(options.string_encoding),
    }
}

fn lift_string_encoding(encoding: EnvironStringEncoding) -> StringEncoding {
    match encoding {
        EnvironStringEncoding::Utf8 => StringEncoding::Utf8,
        EnvironStringEncoding::Utf16 => StringEncoding::Utf16,
        EnvironStringEncoding::CompactUtf16 => StringEncoding::CompactUtf16,
    }
}

fn lift_entity_index(idx: EnvironEntityIndex) -> EntityIndex {
    match idx {
        EnvironEntityIndex::Function(i) => EntityIndex::Function(i.as_u32()),
        EnvironEntityIndex::Table(i) => EntityIndex::Table(i.as_u32()),
        EnvironEntityIndex::Memory(i) => EntityIndex::Memory(i.as_u32()),
        EnvironEntityIndex::Global(i) => EntityIndex::Global(i.as_u32()),
        EnvironEntityIndex::Tag(i) => EntityIndex::Tag(i.as_u32()),
    }
}

fn translation_error<E: Display>(err: E) -> Error {
    Error::InvalidComponentBinary {
        message: format!("{err}"),
        offset: 0,
    }
}

fn internal(message: &str) -> Error {
    Error::Internal {
        message: message.to_owned(),
    }
}

fn internal_msg(message: String) -> Error {
    Error::Internal { message }
}
