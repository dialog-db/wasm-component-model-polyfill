//! Native translator: drives [`wasmtime_environ::component::Translator`]
//! over a component binary, then projects the resulting rich IR into
//! the polyfill's own [`ExecutorIr`] shape so the executor's driver
//! is target-agnostic.

use core::fmt::Display;
use std::collections::HashMap;

use wasm_runtime_layer::Module as RuntimeModule;
use wasmtime_environ::component::{
    ComponentTypesBuilder, CoreDef, CoreExport, Export as ComponentExport,
    ExportItem as ComponentExportItem, GlobalInitializer, InstantiateModule,
    StaticModuleIndex, Translator,
};
use wasmtime_environ::wasmparser::{Validator, WasmFeatures};
use wasmtime_environ::{EntityIndex as EnvironEntityIndex, ScopeVec, Tunables};

use crate::engine::Engine;
use crate::error::{Error, Result};

use super::ir::{
    CoreInstanceExport, CoreSourceItem, EntityIndex, ExecutorIr, ExportSpec, ImportSource,
    Initializer, ModuleEntry, ModuleImport,
};

/// Translate `bytes` against `engine` and return the executor's IR.
pub fn translate(engine: &Engine, bytes: &[u8]) -> Result<ExecutorIr> {
    let scope = ScopeVec::new();
    let tunables = Tunables::default_u32();
    let mut validator = Validator::new_with_features(WasmFeatures::all());
    let mut types = ComponentTypesBuilder::new(&validator);

    let (translation, modules) = Translator::new(&tunables, &mut validator, &mut types, &scope)
        .translate(bytes)
        .map_err(translation_error)?;

    let mut module_entries: Vec<ModuleEntry> = Vec::with_capacity(modules.len());
    let mut module_index_for_static: HashMap<StaticModuleIndex, usize> =
        HashMap::with_capacity(modules.len());
    for (static_idx, module) in modules {
        let runtime =
            RuntimeModule::new(engine.inner(), module.wasm).map_err(translation_error)?;
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
            .map(|(atom, idx)| (lift_entity_index(*idx), module.module.strings[*atom].to_owned()))
            .collect();
        module_index_for_static.insert(static_idx, module_entries.len());
        module_entries.push(ModuleEntry {
            runtime,
            imports,
            entity_to_name,
        });
    }

    let mut initializers: Vec<Initializer> = Vec::new();
    let mut runtime_instance_to_module: Vec<usize> = Vec::new();
    for initializer in &translation.component.initializers {
        match initializer {
            GlobalInitializer::InstantiateModule(InstantiateModule::Static(static_idx, defs), _) => {
                let module_index = *module_index_for_static.get(static_idx).ok_or_else(|| {
                    internal("module index from translator missing from projection map")
                })?;
                let mut imports = Vec::with_capacity(defs.len());
                for def in defs.iter() {
                    imports.push(lift_core_def(def, &runtime_instance_to_module)?);
                }
                runtime_instance_to_module.push(module_index);
                initializers.push(Initializer::InstantiateModule {
                    module_index,
                    imports: imports.into_boxed_slice(),
                });
            }
            GlobalInitializer::InstantiateModule(InstantiateModule::Import(_, _), _) => {
                todo!(
                    "import-style core-module instantiation depends on host-item registration, which lands with PDD008"
                )
            }
            GlobalInitializer::LowerImport { .. } => {
                todo!(
                    "lowering host imports requires the host-function registration that lands with PDD008"
                )
            }
            GlobalInitializer::ExtractMemory(_)
            | GlobalInitializer::ExtractRealloc(_)
            | GlobalInitializer::ExtractCallback(_)
            | GlobalInitializer::ExtractPostReturn(_)
            | GlobalInitializer::ExtractTable(_) => {
                todo!(
                    "canonical-ABI runtime state extraction (memory / realloc / callback / post-return / table) lands with the compound-valtype lift/lower work"
                )
            }
            GlobalInitializer::Resource(_) => {
                todo!("host-resource registration lands with PDD009")
            }
        }
    }

    let mut exports: Vec<ExportSpec> = Vec::new();
    for (name, export_index) in translation.component.exports.raw_iter() {
        let export = translation
            .component
            .export_items
            .get(*export_index)
            .ok_or_else(|| internal("export index from translator missing from export_items"))?;
        match export {
            ComponentExport::LiftedFunction { func, .. } => {
                let source = lift_core_def(func, &runtime_instance_to_module)?;
                exports.push(ExportSpec {
                    name: name.clone(),
                    source,
                });
            }
            ComponentExport::ModuleStatic { .. } | ComponentExport::ModuleImport { .. } => {
                todo!(
                    "module-typed component exports — out of scope for the synchronous baseline; see PDD003 checklist"
                )
            }
            ComponentExport::Instance { .. } => {
                todo!(
                    "instance-typed component exports — the polyfill exposes function exports only at present"
                )
            }
            ComponentExport::Type(_) => {
                // Type exports carry no runtime presence.
            }
        }
    }

    Ok(ExecutorIr {
        modules: module_entries.into_boxed_slice(),
        initializers: initializers.into_boxed_slice(),
        exports: exports.into_boxed_slice(),
    })
}

fn lift_core_def(
    def: &CoreDef,
    runtime_instance_to_module: &[usize],
) -> Result<ImportSource> {
    match def {
        CoreDef::Export(export) => lift_core_export(export, runtime_instance_to_module),
        CoreDef::Trampoline(_) => todo!(
            "trampoline-backed core definitions depend on host-function or host-resource registration that lands with PDD008/PDD009"
        ),
        CoreDef::InstanceFlags(_) => todo!(
            "component-instance flag globals are part of the canonical-ABI runtime state that lands with the compound-valtype lift/lower work"
        ),
        CoreDef::UnsafeIntrinsic(_) => todo!(
            "Wasmtime unsafe intrinsics are not part of the polyfill's surface — see PDD003"
        ),
        CoreDef::TaskMayBlock => todo!(
            "task-may-block global is part of the async-tier runtime state — see PDD003"
        ),
    }
}

fn lift_core_export<U: Copy + Into<EnvironEntityIndex>>(
    export: &CoreExport<U>,
    runtime_instance_to_module: &[usize],
) -> Result<ImportSource> {
    let _ = runtime_instance_to_module
        .get(export.instance.as_u32() as usize)
        .ok_or_else(|| internal("CoreExport instance index missing from projection map"))?;
    let item = match &export.item {
        ComponentExportItem::Name(s) => CoreSourceItem::Name(s.clone()),
        ComponentExportItem::Index(idx) => CoreSourceItem::Index(lift_entity_index((*idx).into())),
    };
    Ok(ImportSource::CoreInstanceExport(CoreInstanceExport {
        instance_index: export.instance.as_u32() as usize,
        item,
    }))
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
