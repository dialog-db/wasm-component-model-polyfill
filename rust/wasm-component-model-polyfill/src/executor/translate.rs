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

use wasmtime_environ::component::{
    CanonicalOptions as EnvironCanonOptions, CanonicalOptionsDataModel, ComponentTranslation,
    ComponentTypes, ComponentTypesBuilder, CoreDef, CoreExport, Export as EnvironExport,
    ExportIndex, ExportItem as EnvironExportItem, ExtractMemory, ExtractPostReturn, ExtractRealloc,
    FixedEncoding, GlobalInitializer, InstantiateModule, LoweredIndex, RuntimeImportIndex,
    StaticModuleIndex, StringEncoding as EnvironStringEncoding, Trampoline, TrampolineIndex,
    Transcode, Translator, TypeResourceTable, TypeResourceTableIndex, UnsafeIntrinsic,
};
use wasmtime_environ::prelude::Error as TranslatorError;
use wasmtime_environ::wasmparser::Validator;
use wasmtime_environ::{
    EntityIndex as EnvironEntityIndex, ScopeVec, Tunables, WasmError, WasmValType,
};

use crate::abi::layout::FlatType;

use crate::component::{ComponentExport, ComponentImport, ExternType, ExternalName, TypeProjector};
use crate::engine::Engine;
use crate::error::{Error, Result};

use crate::module::Module;

use super::ir::{
    CanonOptions, CoreInstanceExport, CoreSignature, CoreSourceItem, DataModel, EntityIndex,
    ExecutorIr, ExportSpec, ImportSource, Initializer, LoweringSpec, ModuleEntry, ModuleExportSpec,
    ModuleSource, NamedImportSource, ResourceSpec, ResourceTableSpec, StringEncoding,
    TrampolineSpec, TranscodeOp,
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

/// Translate `bytes` against `engine`. The future suspends only while
/// the browser compiles a core module; on native it completes without
/// suspending.
pub async fn translate(engine: &Engine, bytes: &[u8]) -> Result<Translation> {
    let scope = ScopeVec::new();
    // The translator's defaults keep concurrency support on. Turning
    // it off makes the fused adapter compiler assert on an `async`
    // function instead of reporting it, so the polyfill leaves it on
    // and provides the `task_may_block` global synchronous adapters
    // import under that setting.
    let tunables = Tunables::default_u32();
    let mut validator = Validator::new_with_features(engine.config().wasm_features());
    let mut types = ComponentTypesBuilder::new(&validator);

    let (translation, modules) = Translator::new(&tunables, &mut validator, &mut types, &scope)
        .translate(bytes)
        .map_err(translation_error)?;

    // The builder knows how many resource tables the component has;
    // the finished types index them but do not count them.
    let num_resource_tables = types.num_resource_tables();
    let (component_types, _) = types.finish(&translation.component);
    let projector = TypeProjector::new(&component_types, &translation.component);

    let imports = project_imports(&translation, &projector)?;
    let exports = project_exports(&translation, &projector)?;

    let mut module_entries: Vec<ModuleEntry> = Vec::with_capacity(modules.len());
    let mut module_index_for_static: HashMap<StaticModuleIndex, usize> =
        HashMap::with_capacity(modules.len());
    for (static_idx, module) in modules {
        let compiled = Module::new(engine, module.wasm).await?;
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
            module: compiled,
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

    // Pre-walk #2: one `ResourceSpec` per resource, at the
    // translator's resource index: imported resources first, then the
    // ones the component defines. A defined resource's destructor is
    // filled in when its initializer is reached below.
    let mut resources: Vec<ResourceSpec> = Vec::new();
    for (_, runtime_import) in translation.component.imported_resources.iter() {
        let (import_index, path) = import_path(&translation, *runtime_import)?;
        resources.push(ResourceSpec::Imported { import_index, path });
    }
    for (_, instance) in translation.component.defined_resource_instances.iter() {
        resources.push(ResourceSpec::Local {
            instance: instance.as_u32() as usize,
            destructor: None,
        });
    }

    // Pre-walk #2b: one entry per resource table, at the translator's
    // table index. A concrete table names its resource and the instance
    // that keeps it; an abstract one has no runtime presence.
    let resource_tables: Vec<Option<ResourceTableSpec>> = (0..num_resource_tables as u32)
        .map(
            |i| match &component_types[TypeResourceTableIndex::from_u32(i)] {
                TypeResourceTable::Concrete { ty, instance } => {
                    let defining = translation
                        .component
                        .defined_resource_index(*ty)
                        .map(|defined| {
                            translation.component.defined_resource_instances[defined] == *instance
                        })
                        .unwrap_or(false);
                    Some(ResourceTableSpec {
                        resource_index: ty.as_u32() as usize,
                        instance: instance.as_u32() as usize,
                        defining,
                    })
                }
                TypeResourceTable::Abstract(_) => None,
            },
        )
        .collect();

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
                let (import_index, path) = import_path(&translation, runtime_import)?;
                TrampolineSpec::LowerImport(LoweringSpec {
                    import_index,
                    path,
                    signature: projector.function(*lower_ty)?,
                    options: lift_canon_options(canon)?,
                })
            }
            Trampoline::ResourceDrop { ty, .. } => TrampolineSpec::ResourceDrop {
                table_index: resolve_table_index(&component_types, resource_tables.len(), *ty)?,
            },
            Trampoline::ResourceNew { ty, .. } => TrampolineSpec::ResourceNew {
                table_index: resolve_table_index(&component_types, resource_tables.len(), *ty)?,
            },
            Trampoline::ResourceRep { ty, .. } => TrampolineSpec::ResourceRep {
                table_index: resolve_table_index(&component_types, resource_tables.len(), *ty)?,
            },
            // A 64-bit memory on either side shows in the core
            // signature the translator recorded: the transcoder reads
            // its pointers and lengths at the width of each slot.
            Trampoline::Transcoder { op, from, to, .. } => TrampolineSpec::Transcoder {
                op: lift_transcode_op(*op),
                from_memory: from.as_u32() as usize,
                to_memory: to.as_u32() as usize,
                signature: core_signature(&component_types, &translation, trampoline_idx)?,
            },
            Trampoline::ResourceTransferOwn => TrampolineSpec::ResourceTransferOwn {
                signature: core_signature(&component_types, &translation, trampoline_idx)?,
            },
            Trampoline::ResourceTransferBorrow => TrampolineSpec::ResourceTransferBorrow {
                signature: core_signature(&component_types, &translation, trampoline_idx)?,
            },
            Trampoline::Trap => TrampolineSpec::Trap {
                signature: core_signature(&component_types, &translation, trampoline_idx)?,
            },
            Trampoline::EnterSyncCall => TrampolineSpec::EnterSyncCall {
                signature: core_signature(&component_types, &translation, trampoline_idx)?,
            },
            Trampoline::ExitSyncCall => TrampolineSpec::ExitSyncCall {
                signature: core_signature(&component_types, &translation, trampoline_idx)?,
            },
            // Concurrency built-ins are not built. A
            // `CoreDef::Trampoline` that references one surfaces
            // `Error::Unsupported` in `lift_core_def`.
            _ => continue,
        };
        let slot = trampoline_specs.len();
        trampoline_specs.push(spec);
        trampoline_to_spec.insert(trampoline_idx, slot);
    }

    // Main walk. Intrinsics that appear as `CoreDef`s rather than
    // as trampolines get their specs appended after the pre-built
    // ones, so the state knows where its own entries start.
    let mut state = ProjectionState::new(trampoline_specs.len());

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
                state.runtime_instance_to_module.push(Some(module_index));
                state.initializers.push(Initializer::InstantiateModule {
                    module_index,
                    imports: imports.into_boxed_slice(),
                });
            }
            GlobalInitializer::InstantiateModule(InstantiateModule::Import(import, args), _) => {
                let (import_index, path) = import_path(&translation, *import)?;
                let mut imports = Vec::new();
                for (module, items) in args.iter() {
                    for (name, def) in items.iter() {
                        imports.push(NamedImportSource {
                            module: module.clone(),
                            name: name.clone(),
                            source: state.lift_core_def(def, &trampoline_to_spec)?,
                        });
                    }
                }
                state.runtime_instance_to_module.push(None);
                state
                    .initializers
                    .push(Initializer::InstantiateImportedModule {
                        source: ModuleSource::Import { import_index, path },
                        imports: imports.into_boxed_slice(),
                    });
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
            GlobalInitializer::Resource(resource) => {
                if resource.rep != WasmValType::I32 {
                    return Err(Error::unsupported(
                        "resource representations other than i32",
                    ));
                }
                let slot = translation
                    .component
                    .resource_index(resource.index)
                    .as_u32() as usize;
                let destructor = resource
                    .dtor
                    .as_ref()
                    .map(|def| state.lift_core_def(def, &trampoline_to_spec))
                    .transpose()?;
                match resources.get_mut(slot) {
                    Some(ResourceSpec::Local {
                        destructor: slot_destructor,
                        ..
                    }) => *slot_destructor = destructor,
                    _ => {
                        return Err(Error::internal(
                            "resource initializer names a resource the pre-walk did not define",
                        ));
                    }
                }
                state.initializers.push(Initializer::DefineResource {
                    resource_index: slot,
                });
            }
        }
    }

    let mut export_tree = ExportTree {
        functions: Vec::new(),
        instances: Vec::new(),
        modules: Vec::new(),
        module_index_for_static: &module_index_for_static,
    };
    for (name, (export_index, _)) in translation.component.exports.raw_iter() {
        collect_export_spec(
            &translation,
            &projector,
            &mut state,
            &trampoline_to_spec,
            &mut export_tree,
            &[],
            name,
            *export_index,
        )?;
    }

    trampoline_specs.append(&mut state.extra_specs);

    Ok(Translation {
        imports,
        exports,
        ir: ExecutorIr {
            modules: module_entries.into_boxed_slice(),
            initializers: state.initializers.into_boxed_slice(),
            exports: export_tree.functions.into_boxed_slice(),
            instance_exports: export_tree.instances.into_boxed_slice(),
            module_exports: export_tree.modules.into_boxed_slice(),
            trampoline_specs: trampoline_specs.into_boxed_slice(),
            resources: resources.into_boxed_slice(),
            resource_tables: resource_tables.into_boxed_slice(),
            runtime_instance_to_module: state.runtime_instance_to_module.into_boxed_slice(),
            num_runtime_memories: state.num_runtime_memories,
            num_runtime_reallocs: state.num_runtime_reallocs,
            num_runtime_post_returns: state.num_runtime_post_returns,
            num_component_instances: translation.component.num_runtime_component_instances as usize,
        },
    })
}

/// The core signature the translator recorded for a trampoline.
fn core_signature(
    component_types: &ComponentTypes,
    translation: &ComponentTranslation,
    index: TrampolineIndex,
) -> Result<CoreSignature> {
    let interned = *translation
        .component
        .trampolines
        .get(index)
        .ok_or_else(|| Error::internal("trampoline index has no core type"))?;
    let func = component_types.module_types()[interned].unwrap_func();
    let mut params = Vec::with_capacity(func.params().len());
    for ty in func.params() {
        params.push(lift_core_val_type(ty)?);
    }
    let mut results = Vec::with_capacity(func.results().len());
    for ty in func.results() {
        results.push(lift_core_val_type(ty)?);
    }
    Ok(CoreSignature { params, results })
}

fn lift_core_val_type(ty: &WasmValType) -> Result<FlatType> {
    Ok(match ty {
        WasmValType::I32 => FlatType::I32,
        WasmValType::I64 => FlatType::I64,
        WasmValType::F32 => FlatType::F32,
        WasmValType::F64 => FlatType::F64,
        WasmValType::V128 | WasmValType::Ref(_) => {
            return Err(Error::unsupported(
                "vector or reference types in intrinsic signatures",
            ));
        }
    })
}

fn lift_transcode_op(op: Transcode) -> TranscodeOp {
    match op {
        Transcode::Copy(FixedEncoding::Utf8) => TranscodeOp::CopyUtf8,
        Transcode::Copy(FixedEncoding::Utf16) => TranscodeOp::CopyUtf16,
        Transcode::Copy(FixedEncoding::Latin1) => TranscodeOp::CopyLatin1,
        Transcode::Latin1ToUtf16 => TranscodeOp::Latin1ToUtf16,
        Transcode::Latin1ToUtf8 => TranscodeOp::Latin1ToUtf8,
        Transcode::Utf16ToCompactProbablyUtf16 => TranscodeOp::Utf16ToCompactProbablyUtf16,
        Transcode::Utf16ToCompactUtf16 => TranscodeOp::Utf16ToCompactUtf16,
        Transcode::Utf16ToLatin1 => TranscodeOp::Utf16ToLatin1,
        Transcode::Utf16ToUtf8 => TranscodeOp::Utf16ToUtf8,
        Transcode::Utf8ToCompactUtf16 => TranscodeOp::Utf8ToCompactUtf16,
        Transcode::Utf8ToLatin1 => TranscodeOp::Utf8ToLatin1,
        Transcode::Utf8ToUtf16 => TranscodeOp::Utf8ToUtf16,
    }
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
            EnvironExport::ModuleStatic { ty, .. } | EnvironExport::ModuleImport { ty, .. } => {
                ExternType::Module(projector.module(*ty)?)
            }
        };
        exports.push(ComponentExport {
            name: ExternalName::from_raw(name),
            ty,
        });
    }
    Ok(exports.into_boxed_slice())
}

/// The export entries the translator collects: one [`ExportSpec`]
/// per lifted function at any depth, the path of every instance-typed
/// export, and one [`ModuleExportSpec`] per module-typed export.
struct ExportTree<'a> {
    functions: Vec<ExportSpec>,
    instances: Vec<Box<[ExternalName]>>,
    modules: Vec<ModuleExportSpec>,
    /// The polyfill's module slot for each static module the
    /// translator numbered.
    module_index_for_static: &'a HashMap<StaticModuleIndex, usize>,
}

/// Recursively project one component-level export into the
/// [`ExportTree`]. A lifted function lands as one [`ExportSpec`]
/// carrying the `path` of the instance exports that enclose it; a
/// static core module lands as one [`ModuleExportSpec`]; an
/// instance-typed export records its own path and recurses into its
/// items with that path threaded through. Any name is accepted for an
/// instance export, a WIT interface identifier or a plain label, as
/// Wasmtime accepts it.
#[allow(clippy::too_many_arguments)]
fn collect_export_spec(
    translation: &ComponentTranslation,
    projector: &TypeProjector<'_>,
    state: &mut ProjectionState,
    trampoline_to_spec: &HashMap<TrampolineIndex, usize>,
    out: &mut ExportTree<'_>,
    path: &[ExternalName],
    name: &str,
    export_index: ExportIndex,
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
            out.functions.push(ExportSpec {
                name: name.to_owned(),
                path: path.into(),
                source,
                signature: projector.function(*ty)?,
                options: lift_canon_options(canon)?,
            });
            Ok(())
        }
        EnvironExport::ModuleStatic { index, .. } => {
            let module_index = *out.module_index_for_static.get(index).ok_or_else(|| {
                Error::internal("module export names a static module the translator did not emit")
            })?;
            out.modules.push(ModuleExportSpec {
                name: name.to_owned(),
                path: path.into(),
                source: ModuleSource::Static(module_index),
            });
            Ok(())
        }
        EnvironExport::ModuleImport { import, .. } => {
            let (import_index, item_path) = import_path(translation, *import)?;
            out.modules.push(ModuleExportSpec {
                name: name.to_owned(),
                path: path.into(),
                source: ModuleSource::Import {
                    import_index,
                    path: item_path,
                },
            });
            Ok(())
        }
        EnvironExport::Instance { exports, .. } => {
            let mut nested = path.to_vec();
            nested.push(ExternalName::from_raw(name));
            out.instances.push(nested.clone().into_boxed_slice());
            for (item_name, (inner_index, _)) in exports.raw_iter() {
                collect_export_spec(
                    translation,
                    projector,
                    state,
                    trampoline_to_spec,
                    out,
                    &nested,
                    item_name,
                    *inner_index,
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

/// Resolve a [`TypeResourceTableIndex`] to the polyfill's table
/// index: the same number, checked to name a concrete table.
fn resolve_table_index(
    component_types: &ComponentTypes,
    num_tables: usize,
    ty: TypeResourceTableIndex,
) -> Result<usize> {
    if let TypeResourceTable::Abstract(_) = &component_types[ty] {
        return Err(Error::internal(
            "resource trampoline references an abstract resource table in a concrete instantiation",
        ));
    }
    let index = ty.as_u32() as usize;
    if index < num_tables {
        Ok(index)
    } else {
        Err(Error::internal(
            "resource trampoline references a table outside the component's resource tables",
        ))
    }
}

/// Resolve a runtime import to the polyfill import index and the
/// item path inside it: one name per nesting level from the imported
/// instance down to the item, or empty when the import is the item.
/// The polyfill's import list is the translator's `import_types` in
/// order, so the import index is the translator's.
fn import_path(
    translation: &ComponentTranslation,
    runtime_import: RuntimeImportIndex,
) -> Result<(usize, Box<[String]>)> {
    let (import_idx, path) = translation
        .component
        .imports
        .get(runtime_import)
        .ok_or_else(|| Error::internal("RuntimeImportIndex out of bounds"))?;
    Ok((
        import_idx.as_u32() as usize,
        path.clone().into_boxed_slice(),
    ))
}

/// Working state for the main initializer walk. Carries only the
/// data the main walk produces; trampoline/lowering tables are
/// pre-built and read-only at this stage.
struct ProjectionState {
    runtime_instance_to_module: Vec<Option<usize>>,
    initializers: Vec<Initializer>,
    num_runtime_memories: usize,
    num_runtime_reallocs: usize,
    num_runtime_post_returns: usize,
    /// Trampoline specs created on demand for intrinsics that appear
    /// as `CoreDef`s. Their indices start at `spec_base`.
    extra_specs: Vec<TrampolineSpec>,
    spec_base: usize,
    intrinsic_to_spec: HashMap<UnsafeIntrinsic, usize>,
}

impl ProjectionState {
    fn new(spec_base: usize) -> Self {
        Self {
            runtime_instance_to_module: Vec::new(),
            initializers: Vec::new(),
            num_runtime_memories: 0,
            num_runtime_reallocs: 0,
            num_runtime_post_returns: 0,
            extra_specs: Vec::new(),
            spec_base,
            intrinsic_to_spec: HashMap::new(),
        }
    }

    /// The trampoline index of the spec for `intrinsic`, creating
    /// it on first use.
    fn intrinsic(&mut self, intrinsic: UnsafeIntrinsic) -> Result<usize> {
        if let Some(index) = self.intrinsic_to_spec.get(&intrinsic) {
            return Ok(*index);
        }
        let signature = CoreSignature {
            params: intrinsic
                .core_params()
                .iter()
                .map(lift_core_val_type)
                .collect::<Result<Vec<_>>>()?,
            results: intrinsic
                .core_results()
                .iter()
                .map(lift_core_val_type)
                .collect::<Result<Vec<_>>>()?,
        };
        let spec = match intrinsic {
            UnsafeIntrinsic::ContextGetI32_0 => TrampolineSpec::ContextGet { slot: 0, signature },
            UnsafeIntrinsic::ContextGetI32_1 => TrampolineSpec::ContextGet { slot: 1, signature },
            UnsafeIntrinsic::ContextSetI32_0 => TrampolineSpec::ContextSet { slot: 0, signature },
            UnsafeIntrinsic::ContextSetI32_1 => TrampolineSpec::ContextSet { slot: 1, signature },
            other => {
                return Err(Error::unsupported(format!(
                    "the `{}` intrinsic",
                    other.name()
                )));
            }
        };
        let index = self.spec_base + self.extra_specs.len();
        self.extra_specs.push(spec);
        self.intrinsic_to_spec.insert(intrinsic, index);
        Ok(index)
    }

    fn lift_core_def(
        &mut self,
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
            CoreDef::InstanceFlags(instance) => {
                Ok(ImportSource::InstanceFlags(instance.as_u32() as usize))
            }
            CoreDef::UnsafeIntrinsic(intrinsic) => {
                Ok(ImportSource::Trampoline(self.intrinsic(*intrinsic)?))
            }
            CoreDef::TaskMayBlock => Ok(ImportSource::TaskMayBlock),
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

/// Project one canon-options bundle onto the polyfill's own.
///
/// The garbage-collected data model reaches the field faithfully and
/// is then refused: the polyfill implements the linear-memory ABI
/// strategy alone, and an option it does not implement fails here
/// rather than at the crossing that would have used it.
fn lift_canon_options(options: &EnvironCanonOptions) -> Result<CanonOptions> {
    let (realloc, data_model) = match options.data_model {
        CanonicalOptionsDataModel::LinearMemory(opts) => (
            opts.realloc.map(|i| i.as_u32() as usize),
            DataModel::LinearMemory,
        ),
        CanonicalOptionsDataModel::Gc {} => (None, DataModel::Gc),
    };
    if data_model != DataModel::LinearMemory {
        return Err(Error::unsupported(
            "the garbage-collected canonical-ABI data model",
        ));
    }
    Ok(CanonOptions {
        instance: options.instance.as_u32() as usize,
        memory: options.memory().map(|i| i.as_u32() as usize),
        realloc,
        post_return: options.post_return.map(|i| i.as_u32() as usize),
        string_encoding: lift_string_encoding(options.string_encoding),
        data_model,
    })
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
