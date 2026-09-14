//! Component translator.
//!
//! Drives [`wasmtime_environ::component::Translator`] over a
//! component binary exactly once, compiles every core module the
//! binary contains against the runtime layer, projects the
//! translator's type information onto the polyfill's data shapes,
//! and projects the translator's rich IR into the polyfill's own
//! [`ExecutorIr`] so the executor's driver is target-agnostic. The
//! same translator runs on every supported target; see [`super`]
//! for why the `compile` feature builds cleanly on
//! `wasm32-unknown-unknown`.

use std::collections::HashMap;

use wasm_runtime_layer::Module as RuntimeModule;
use wasmtime_environ::component::{
    CanonicalOptions as EnvironCanonOptions, CanonicalOptionsDataModel, ComponentTranslation,
    ComponentTypes, ComponentTypesBuilder, CoreDef, CoreExport, Export as EnvironExport,
    ExportIndex, ExportItem as EnvironExportItem, ExtractMemory, ExtractPostReturn, ExtractRealloc,
    GlobalInitializer, InstantiateModule, LoweredIndex, ResourceIndex, RuntimeImportIndex,
    StaticModuleIndex, StringEncoding as EnvironStringEncoding, Trampoline, TrampolineIndex,
    Translator, TypeResourceTable, TypeResourceTableIndex,
};
use wasmtime_environ::prelude::Error as TranslatorError;
use wasmtime_environ::wasmparser::{Validator, WasmFeatures};
use wasmtime_environ::{EntityIndex as EnvironEntityIndex, ScopeVec, Tunables, WasmError};

use crate::component::{ComponentExport, ComponentImport, ExternType, ExternalName, TypeProjector};
use crate::engine::Engine;
use crate::error::{Error, InstantiationError, Result};
use crate::identifier::InterfaceIdentifier;

use super::ir::{
    CanonOptions, CoreInstanceExport, CoreSourceItem, EntityIndex, ExecutorIr, ExportSpec,
    ImportSource, Initializer, LoweringSpec, ModuleEntry, ModuleImport, ResourceSpec,
    StringEncoding, TrampolineSpec,
};

/// Everything one translation of a component binary produces.
pub struct Translation {
    /// The declared imports, in declaration order.
    pub imports: Box<[ComponentImport]>,
    /// The declared exports, in declaration order.
    pub exports: Box<[ComponentExport]>,
    /// The executor's plan, with every core module compiled.
    pub ir: ExecutorIr,
}

/// Translate `bytes` against `engine`.
pub fn translate(engine: &Engine, bytes: &[u8]) -> Result<Translation> {
    let scope = ScopeVec::new();
    let tunables = Tunables::default_u32();
    let mut validator = Validator::new_with_features(WasmFeatures::all());
    let mut types = ComponentTypesBuilder::new(&validator);

    let (translation, modules) = Translator::new(&tunables, &mut validator, &mut types, &scope)
        .translate(bytes)
        .map_err(translation_error)?;

    let (component_types, _) = types.finish(&translation.component);
    let projector = TypeProjector::new(&component_types, &translation.component);

    let imports = project_imports(&translation, &projector)?;
    let exports = project_exports(&translation, &projector)?;

    let mut module_entries: Vec<ModuleEntry> = Vec::with_capacity(modules.len());
    let mut module_index_for_static: HashMap<StaticModuleIndex, usize> =
        HashMap::with_capacity(modules.len());
    for (static_idx, module) in modules {
        // The translator already validated the module; a compile
        // failure here means the runtime layer refused a valid
        // module, which is a substrate concern.
        let runtime = RuntimeModule::new(engine.inner(), module.wasm)
            .map_err(InstantiationError::SubstrateFailure)
            .map_err(Error::from)?;
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
    // map to pair its options with the right import.
    let mut lowered_to_import: HashMap<LoweredIndex, RuntimeImportIndex> = HashMap::new();
    for initializer in &translation.component.initializers {
        if let GlobalInitializer::LowerImport { index, import } = initializer {
            lowered_to_import.insert(*index, *import);
        }
    }

    // Pre-walk #2: project every imported resource into a
    // `ResourceSpec`.
    let mut resources: Vec<ResourceSpec> = Vec::new();
    let mut resource_to_spec: HashMap<ResourceIndex, usize> = HashMap::new();
    for (resource_idx, runtime_import) in translation.component.imported_resources.iter() {
        let (import_index, item_name) = import_path(&translation, *runtime_import)?;
        let slot = resources.len();
        resources.push(ResourceSpec {
            import_index,
            item_name,
        });
        resource_to_spec.insert(resource_idx, slot);
    }

    // Pre-walk #3: build a `TrampolineSpec` for every trampoline
    // kind the polyfill implements. The resulting indices are
    // consulted by `CoreDef::Trampoline` resolution.
    let mut trampoline_specs: Vec<TrampolineSpec> = Vec::new();
    let mut trampoline_to_spec: HashMap<TrampolineIndex, usize> = HashMap::new();
    for (trampoline_idx, trampoline) in translation.trampolines.iter() {
        let spec = match trampoline {
            Trampoline::LowerImport {
                index: lowered_idx,
                lower_ty,
                options,
            } => {
                let runtime_import = *lowered_to_import.get(lowered_idx).ok_or_else(|| {
                    Error::internal(
                        "Trampoline::LowerImport has no matching GlobalInitializer::LowerImport",
                    )
                })?;
                let canon = translation
                    .component
                    .options
                    .get(*options)
                    .ok_or_else(|| Error::internal("Trampoline OptionsIndex out of bounds"))?;
                let (import_index, item_name) = import_path(&translation, runtime_import)?;
                TrampolineSpec::LowerImport(LoweringSpec {
                    import_index,
                    item_name,
                    signature: projector.function(*lower_ty)?,
                    options: lift_canon_options(canon),
                })
            }
            Trampoline::ResourceDrop { ty, .. } => TrampolineSpec::ResourceDrop {
                resource_index: resolve_resource_index(&component_types, &resource_to_spec, *ty)?,
            },
            Trampoline::ResourceNew { ty, .. } => TrampolineSpec::ResourceNew {
                resource_index: resolve_resource_index(&component_types, &resource_to_spec, *ty)?,
            },
            Trampoline::ResourceRep { ty, .. } => TrampolineSpec::ResourceRep {
                resource_index: resolve_resource_index(&component_types, &resource_to_spec, *ty)?,
            },
            // Other trampoline kinds (string transcoders, resource
            // transfer between components, concurrency built-ins)
            // are not built. A `CoreDef::Trampoline` that references
            // one surfaces `Error::Unsupported` in `lift_core_def`.
            _ => continue,
        };
        let slot = trampoline_specs.len();
        trampoline_specs.push(spec);
        trampoline_to_spec.insert(trampoline_idx, slot);
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
                    Error::internal("module index from translator missing from projection map")
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
                return Err(Error::unsupported(
                    "instantiation of an imported core module",
                ));
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
            GlobalInitializer::ExtractCallback(_) => {
                return Err(Error::unsupported("asynchronous lifts (callback)"));
            }
            GlobalInitializer::ExtractTable(_) => {
                return Err(Error::unsupported("thread built-ins (table extraction)"));
            }
            GlobalInitializer::Resource(_) => {
                return Err(Error::unsupported("locally-defined resources"));
            }
        }
    }

    let mut export_specs: Vec<ExportSpec> = Vec::new();
    for (name, (export_index, _)) in translation.component.exports.raw_iter() {
        collect_export_spec(
            &translation,
            &projector,
            &mut state,
            &trampoline_to_spec,
            &mut export_specs,
            name,
            *export_index,
            None,
        )?;
    }

    Ok(Translation {
        imports,
        exports,
        ir: ExecutorIr {
            modules: module_entries.into_boxed_slice(),
            initializers: state.initializers.into_boxed_slice(),
            exports: export_specs.into_boxed_slice(),
            trampoline_specs: trampoline_specs.into_boxed_slice(),
            resources: resources.into_boxed_slice(),
            runtime_instance_to_module: state.runtime_instance_to_module.into_boxed_slice(),
            num_runtime_memories: state.num_runtime_memories,
            num_runtime_reallocs: state.num_runtime_reallocs,
            num_runtime_post_returns: state.num_runtime_post_returns,
        },
    })
}

/// Project the component's declared imports, in declaration order.
/// The position of each entry is the `import_index` the executor's
/// specs refer to.
fn project_imports(
    translation: &ComponentTranslation,
    projector: &TypeProjector<'_>,
) -> Result<Box<[ComponentImport]>> {
    let mut imports = Vec::with_capacity(translation.component.import_types.len());
    for (_, (name, extern_)) in translation.component.import_types.iter() {
        imports.push(ComponentImport {
            name: ExternalName::from_raw(name),
            ty: projector.extern_type(name, extern_)?,
        });
    }
    Ok(imports.into_boxed_slice())
}

/// Project the component's declared exports, in declaration order.
fn project_exports(
    translation: &ComponentTranslation,
    projector: &TypeProjector<'_>,
) -> Result<Box<[ComponentExport]>> {
    let mut exports = Vec::new();
    for (name, (export_index, _)) in translation.component.exports.raw_iter() {
        let ty = match &translation.component.export_items[*export_index] {
            EnvironExport::LiftedFunction { ty, .. } => {
                ExternType::Function(projector.function(*ty)?)
            }
            EnvironExport::Instance { ty, .. } => ExternType::Instance(projector.instance(*ty)?),
            EnvironExport::Type(def) => projector.type_def(name, def)?,
            EnvironExport::ModuleStatic { .. } | EnvironExport::ModuleImport { .. } => {
                ExternType::Module
            }
        };
        exports.push(ComponentExport {
            name: ExternalName::from_raw(name),
            ty,
        });
    }
    Ok(exports.into_boxed_slice())
}

/// Recursively project one component-level export into
/// [`ExportSpec`] entries. Root-level functions land as a single
/// entry with no parent; instance-typed exports recurse into their
/// inner exports with the enclosing instance's
/// [`InterfaceIdentifier`] threaded through as the `parent`.
#[allow(clippy::too_many_arguments)]
fn collect_export_spec(
    translation: &ComponentTranslation,
    projector: &TypeProjector<'_>,
    state: &mut ProjectionState,
    trampoline_to_spec: &HashMap<TrampolineIndex, usize>,
    out: &mut Vec<ExportSpec>,
    name: &str,
    export_index: ExportIndex,
    parent: Option<&InterfaceIdentifier>,
) -> Result<()> {
    let export = translation
        .component
        .export_items
        .get(export_index)
        .ok_or_else(|| Error::internal("export index from translator missing from export_items"))?;
    match export {
        EnvironExport::LiftedFunction { ty, func, options } => {
            let source = state.lift_core_def(func, trampoline_to_spec)?;
            let canon = translation
                .component
                .options
                .get(*options)
                .ok_or_else(|| Error::internal("export OptionsIndex out of bounds"))?;
            out.push(ExportSpec {
                name: name.to_owned(),
                parent: parent.cloned(),
                source,
                signature: projector.function(*ty)?,
                options: lift_canon_options(canon),
            });
            Ok(())
        }
        EnvironExport::ModuleStatic { .. } | EnvironExport::ModuleImport { .. } => {
            Err(Error::unsupported("module-typed exports"))
        }
        EnvironExport::Instance { exports, .. } => {
            if parent.is_some() {
                return Err(Error::unsupported(
                    "instance exports nested more than one level",
                ));
            }
            let identifier = name.parse::<InterfaceIdentifier>().map_err(|_| {
                Error::unsupported(format!(
                    "instance exports with a plain name (`{name}` is not a WIT interface identifier)"
                ))
            })?;
            for (item_name, (inner_index, _)) in exports.raw_iter() {
                collect_export_spec(
                    translation,
                    projector,
                    state,
                    trampoline_to_spec,
                    out,
                    item_name,
                    *inner_index,
                    Some(&identifier),
                )?;
            }
            Ok(())
        }
        EnvironExport::Type(_) => {
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
    resource_to_spec: &HashMap<ResourceIndex, usize>,
    ty: TypeResourceTableIndex,
) -> Result<usize> {
    let table = &component_types[ty];
    let resource_idx = match table {
        TypeResourceTable::Concrete { ty, .. } => *ty,
        TypeResourceTable::Abstract(_) => {
            return Err(Error::internal(
                "resource trampoline references an abstract resource table in a concrete instantiation",
            ));
        }
    };
    resource_to_spec
        .get(&resource_idx)
        .copied()
        .ok_or_else(|| Error::unsupported("locally-defined resources"))
}

/// Resolve a runtime import to the polyfill import index and the
/// item path inside it. The polyfill's import list is the
/// translator's `import_types` in order, so the import index is the
/// translator's.
fn import_path(
    translation: &ComponentTranslation,
    runtime_import: RuntimeImportIndex,
) -> Result<(usize, Option<String>)> {
    let (import_idx, path) = translation
        .component
        .imports
        .get(runtime_import)
        .ok_or_else(|| Error::internal("RuntimeImportIndex out of bounds"))?;
    let item_name = match path.len() {
        0 => None,
        1 => Some(path[0].clone()),
        _ => {
            return Err(Error::unsupported("imports nested more than one level"));
        }
    };
    Ok((import_idx.as_u32() as usize, item_name))
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
                    Error::unsupported(
                        "string transcoders, resource transfer, and concurrency built-ins between components",
                    )
                })?;
                Ok(ImportSource::Trampoline(lowering_index))
            }
            CoreDef::InstanceFlags(_) => {
                Err(Error::unsupported("component composition (instance flags)"))
            }
            CoreDef::UnsafeIntrinsic(_) => Err(Error::unsupported("Wasmtime unsafe intrinsics")),
            CoreDef::TaskMayBlock => Err(Error::unsupported("asynchronous lifts (task-may-block)")),
        }
    }

    fn lift_core_export<U: Copy + Into<EnvironEntityIndex>>(
        &self,
        export: &CoreExport<U>,
    ) -> Result<ImportSource> {
        let _ = self
            .runtime_instance_to_module
            .get(export.instance.as_u32() as usize)
            .ok_or_else(|| {
                Error::internal("CoreExport instance index missing from projection map")
            })?;
        let item = match &export.item {
            EnvironExportItem::Name(s) => CoreSourceItem::Name(s.clone()),
            EnvironExportItem::Index(idx) => {
                CoreSourceItem::Index(lift_entity_index((*idx).into()))
            }
        };
        Ok(ImportSource::CoreInstanceExport(CoreInstanceExport {
            instance_index: export.instance.as_u32() as usize,
            item,
        }))
    }
}

fn lift_canon_options(options: &EnvironCanonOptions) -> CanonOptions {
    CanonOptions {
        memory: options.memory().map(|i| i.as_u32() as usize),
        realloc: match options.data_model {
            CanonicalOptionsDataModel::LinearMemory(opts) => {
                opts.realloc.map(|i| i.as_u32() as usize)
            }
            CanonicalOptionsDataModel::Gc {} => None,
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

/// Map a translator failure onto the polyfill's error model. A
/// validation failure keeps the byte offset the translator reports;
/// a feature the translator itself does not support is surfaced as
/// [`Error::Unsupported`].
fn translation_error(err: TranslatorError) -> Error {
    match err.downcast::<WasmError>() {
        Ok(WasmError::InvalidWebAssembly { message, offset }) => {
            Error::InvalidComponentBinary { message, offset }
        }
        Ok(WasmError::Unsupported(feature)) => Error::unsupported(feature),
        Ok(other) => Error::InvalidComponentBinary {
            message: format!("{other}"),
            offset: 0,
        },
        Err(other) => Error::InvalidComponentBinary {
            message: format!("{other:#}"),
            offset: 0,
        },
    }
}
